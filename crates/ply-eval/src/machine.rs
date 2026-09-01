//! The control-stack machine.

use crate::arena::Arena;
use crate::arena::RegionKind;
use crate::builtins::{self, Builtin, Step};
use crate::code::{self, ClosureCode, Code, Lowering, NodeKind, Stmt as CodeStmt, lower};
use crate::cont::{Continuation, Delimiter, Frame, Next, SimId, Stack};
use crate::env::{Env, Slot};
use crate::handler::{self, Answered, Request, Scheduled, Transition};
use crate::host::{
    HostAnswer, HostBinding, HostRequest, HostRuntime, HostUse, MachineId, Pending, attribute,
    err_blocking_answered_inline, err_hermetic, err_host_in_search, err_withheld, operation_label,
};
use crate::limit::{self, DEFAULT_MAX_CALLS, NAMED_CALLS, NESTED_CALLS};
use crate::memo::{Lookup, Memo};
use crate::rc::Own;
use crate::region::{self, Region, StepSite, Trail};
use crate::sched::{HostPolicy, Policy, Resumption, Scheduler, Turn};
use crate::semantics::{
    OpTable, arity_error, ctor_value, err_non_exhaustive, err_not_a_function, err_overflow,
    err_unknown_name, op_decl,
};
use crate::sim::{Access, Answer, DEFAULT_STEPS, Seed};
use crate::task_regions::TaskRegions;
use crate::trace::Trace;
use crate::value::{Closure, ClosureKind, Fields, Value};
use ply_core::CheckOutput;
use ply_core::ty::{EffectAtom, Footprint};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{
    BinOp, Expr, FnDef, Item, Mode, Pattern, PatternKind, Program, QName, TypeDefBody, UnOp,
};
use ply_syntax::resolve::{Namespace, Resolved};
use rustc_hash::FxHashMap;
use std::cell::{Cell, OnceCell};
use std::rc::Rc;
use std::sync::Arc;

// `pub const DEFAULT_MAX_FRAMES: usize = 1_000_000` stood here and was this machine's default frame
// ceiling.
const CALL_SCAN_LIMIT: usize = 4096;

pub enum Progress {
    Running,
    Halted(Value),
}

/// `S` of `⟨S, K, W⟩`.
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

/// Ordered exactly as [`CheckOutput::tests`] is — load order, then source order — because the index
/// into the two is the same index.
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
    /// This machine's identity, which with the performing task is what a host handler keys scoped
    /// state on.
    id: MachineId,
    program: &'a Program,
    resolved: &'a Resolved,
    check: Option<&'a CheckOutput>,
    /// Keyed by program-wide name, so two modules may declare one simple name.
    fns: FxHashMap<Symbol, FnSlot<'a>>,
    /// The lowered form of the definitions this machine has actually called.
    lowered: FxHashMap<Symbol, Value>,
    /// Non-local name resolution, memoised per `(module, qualifier, name)` — keyed by symbols
    /// rather than by `QName`, whose `Eq` includes a `Span` and would miss on every mention after
    /// the first.
    globals: FxHashMap<(usize, Option<Symbol>, Symbol), Value>,
    /// Where the lowering itself is kept, and the reason `lowered` above is only a map from a name
    /// to a closure this machine has already built.
    lowering: Rc<Lowering<'a>>,
    /// The lowered body of the last unlowered closure this machine applied.
    closure_code: ClosureCode,
    /// What a nullary pure definition evaluated to, so a service does not rebuild its route table
    /// once per request.
    memo: Memo,
    ctors: FxHashMap<Symbol, usize>,
    ops: OpTable,
    tests: Vec<TestSlot<'a>>,
    /// Where every cell this engine allocates lives, and the fixture every entry point resets to —
    /// so one seeded fixture serves every test in a run without any of them observing another's
    /// writes.
    regions: TaskRegions,
    /// Which of the region-kind rule's two kinds each region in this program is.
    region_kinds: crate::region_kind::Kinds,
    /// What this entry point performed, which is not what its row said it could.
    trace: Trace,
    stack: Stack,
    state: State,
    current: Span,
    /// An opt-in ceiling on this engine's own heap.
    max_frames: Option<usize>,
    max_calls: usize,
    /// The seed the next entry point's `simulate` region runs at, and the scheduling-step budget
    /// one interleaving may spend.
    seed: Seed,
    sim_steps: u32,
    /// The region currently live.
    sims: Vec<Region>,
    /// How many regions this entry point has entered.
    entered_sims: u32,
    /// The one choice sequence this entry point makes, across every region it enters.
    trail: Trail,
    /// What this entry point's regions did, built once the last of them ended.
    record: Option<region::Record>,
    /// The handler of last resort.
    binding: Arc<HostBinding>,
    /// What answers a [`HostAnswer::Pending`].
    runtime: Option<Rc<dyn HostRuntime>>,
    /// A source of natively compiled bodies.
    compiled: Option<Rc<dyn crate::Compiled>>,
    /// Native entries taken, calls a backend was offered and declined, and answers refused at the
    /// boundary.
    compiled_entries: Cell<u64>,
    compiled_declines: Cell<u64>,
    compiled_refusals: Cell<u64>,
    /// Which definitions' declared parameter types cannot reach a world handle —
    /// `compiled::Gate::ArgumentType`'s table.
    carried_types: OnceCell<crate::compiled::CarriedTypes>,
    /// At-most-once host operations answered in this entry point.
    host_ops: u64,
    /// What this entry point reached across the boundary, and the authority on whether its green
    /// verdict may be cached.
    host_use: HostUse,
    /// The declared footprint of the entry point about to run.
    declared: Option<Footprint>,
    /// Whether this entry point is one of several runs of the same test.
    re_executed: bool,
    /// The last at-most-once host operation answered, so `E0426` can name the packet it is refusing
    /// to send twice.
    last_linear: Option<HostMark>,
    /// What the runtime reported while closing entry points.
    teardown: Vec<Diagnostic>,
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

    /// Everything the machine needs is derivable from the resolved AST alone, so evaluation can be
    /// exercised without a type-check pass.
    pub fn for_program(program: &'a Program, resolved: &'a Resolved) -> Machine<'a> {
        Machine::build(program, resolved, None)
    }

    fn build(
        program: &'a Program,
        resolved: &'a Resolved,
        check: Option<&'a CheckOutput>,
    ) -> Machine<'a> {
        let mut fns = FxHashMap::default();
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
                    // A law is not a global and not a test: `ply-prove` evaluates its body through
                    // `eval_expr_for_test`, with its binders bound to generated values.
                    Item::Law(_) | Item::Derive(_) | Item::EffectSet(_) => {}
                }
            }
        }

        Machine {
            id: MachineId::next(),
            program,
            resolved,
            check,
            fns,
            lowered: FxHashMap::default(),
            globals: FxHashMap::default(),
            lowering: Rc::new(Lowering::for_program(program)),
            closure_code: ClosureCode::default(),
            memo: Memo::default(),
            ctors,
            ops,
            tests,
            regions: TaskRegions::new(),
            region_kinds: crate::region_kind::Kinds::default(),
            trace: Trace::new(),
            stack: Stack::new(),
            state: State::Halt(Value::Unit),
            current: Span::DUMMY,
            max_frames: None,
            max_calls: DEFAULT_MAX_CALLS,
            seed: Seed::default(),
            sim_steps: DEFAULT_STEPS,
            sims: Vec::new(),
            entered_sims: 0,
            trail: Trail::new(Seed::default()),
            record: None,
            binding: Arc::new(HostBinding::hermetic()),
            runtime: None,
            compiled: None,
            compiled_entries: Cell::new(0),
            compiled_declines: Cell::new(0),
            compiled_refusals: Cell::new(0),
            carried_types: OnceCell::new(),
            host_ops: 0,
            host_use: HostUse::default(),
            declared: None,
            re_executed: false,
            last_linear: None,
            teardown: Vec::new(),
        }
    }

    /// Caps this engine's own heap at `max` pending frames.
    pub fn with_max_frames(mut self, max: usize) -> Machine<'a> {
        self.max_frames = Some(max.max(1));
        self
    }

    pub fn with_max_calls(mut self, max: usize) -> Machine<'a> {
        self.max_calls = max.max(1);
        self
    }

    /// The same bound, set on a machine that already exists.
    ///
    /// [`crate::backend::Reference`] needs it: its inner machine is built once
    /// per run and re-bound per call to whatever the outer machine has left.
    pub fn set_max_calls(&mut self, max: usize) {
        self.max_calls = max.max(1);
    }

    /// The atoms this engine performed at the last entry point.
    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    /// Bind the host boundary.
    pub fn set_host_binding(&mut self, binding: Arc<HostBinding>) {
        self.binding = binding;
    }

    /// The reactor a [`HostAnswer::Pending`] is polled on.
    pub fn set_host_runtime(&mut self, runtime: Rc<dyn HostRuntime>) {
        self.runtime = Some(runtime);
    }

    pub fn host_binding(&self) -> &HostBinding {
        &self.binding
    }

    /// At-most-once host operations answered in this entry point.
    pub fn host_ops(&self) -> u64 {
        self.host_ops
    }

    /// The declared footprint of the entry point about to run.
    pub fn set_declared_footprint(&mut self, footprint: Footprint) {
        self.declared = Some(footprint);
    }

    /// Declare that this entry point is one of several runs of the same test, so that reaching the
    /// host boundary is [`codes::HOST_IN_SIMULATION`] rather than a packet sent once per
    /// interleaving.
    pub fn set_re_executed(&mut self, re_executed: bool) {
        self.re_executed = re_executed;
    }

    /// What the last entry point reached across the boundary, or `None` when it reached none.
    pub fn host_use(&self) -> Option<&HostUse> {
        (!self.host_use.is_empty()).then_some(&self.host_use)
    }

    /// Fix the interleaving the next entry point takes.
    pub fn set_seed(&mut self, seed: Seed, steps: u32) {
        self.seed = seed;
        self.sim_steps = steps.max(1);
    }

    /// What the last entry point's `simulate` regions did, or `None` when it reached none.
    pub fn simulated(&self) -> Option<&region::Record> {
        self.record.as_ref()
    }

    pub fn program(&self) -> &'a Program {
        self.program
    }

    pub fn check(&self) -> Option<&'a CheckOutput> {
        self.check
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

    /// The kind of the region opened at `span`, and `None` when that span opens no region: one the
    /// inference never saw, or a `with_cell[r]` nested inside the `with_region[r]` that already
    /// opened `r`.
    pub fn region_kind(&self, span: Span) -> Option<RegionKind> {
        self.region_kinds().at(span).map(|region| region.kind)
    }

    /// This program's region kinds, inferring them if nothing has yet.
    pub fn region_kinds(&self) -> &crate::region_kind::Regions {
        self.region_kinds
            .get_or_init(|| crate::region_kind::infer(self.program, self.resolved))
    }

    /// The handle to hand another engine built from **this same program**, so the analysis behind
    /// it runs once for the program rather than once per engine.
    pub fn shared_region_kinds(&self) -> crate::region_kind::Kinds {
        crate::region_kind::Kinds::clone(&self.region_kinds)
    }

    /// Take another engine's answer for this program instead of inferring one.
    pub fn share_region_kinds(&mut self, kinds: crate::region_kind::Kinds) {
        self.region_kinds = kinds;
    }

    /// The lowering cache to hand a machine built next over **this same program**, so a body is
    /// lowered once for the program rather than once per machine.
    pub fn share_lowering(&self) -> Rc<Lowering<'a>> {
        Rc::clone(&self.lowering)
    }

    /// Lower into `lowering` rather than into a cache of this machine's own.
    pub fn set_lowering(&mut self, lowering: Rc<Lowering<'a>>) {
        if lowering.describes(self.program) {
            self.lowering = lowering;
        }
    }

    /// Take compiled bodies for this program's definitions, so a call the backend accepts is
    /// entered natively instead of evaluated.
    pub fn set_compiled(&mut self, compiled: Rc<dyn crate::Compiled>) {
        if compiled.describes(self.program) {
            self.compiled = Some(compiled);
        }
    }

    /// Native entries taken and calls declined, over this machine's whole life.
    pub fn compiled_counts(&self) -> (u64, u64) {
        (self.compiled_entries.get(), self.compiled_declines.get())
    }

    /// Answers a backend returned that this boundary refuses — a non-scalar, in practice.
    pub fn compiled_refusals(&self) -> u64 {
        self.compiled_refusals.get()
    }

    /// Every subsequent entry point resets to this stack's fixture rather than to an empty one.
    pub fn set_regions(&mut self, regions: TaskRegions) {
        self.regions = regions;
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
        let (module, source) = (slot.module, slot.body);
        let body = self.lowering.body(source);
        self.drive(body, Env::empty(), module).map(|_| ())
    }

    /// A position in this program is not a position in a [`CheckOutput`]: the incremental front end
    /// reports every module's tests while parsing only some of them, so the two lists agree on
    /// order but not on length.
    pub fn eval_test_in(&mut self, module: &Symbol, ordinal: usize) -> Result<(), Diagnostic> {
        let program = self.program;
        let found = self
            .tests
            .iter()
            .filter(|t| program.modules[t.module].name.as_symbol() == module)
            .nth(ordinal)
            .map(|slot| (slot.module, slot.body));
        let Some((owner, source)) = found else {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("module `{module}` has no test at position {ordinal}"),
            )
            .primary(Span::DUMMY, "this test's module was not parsed")
            .note("run `ply cache clear`, or pass `--no-incremental`"));
        };
        let body = self.lowering.body(source);
        self.drive(body, Env::empty(), owner).map(|_| ())
    }

    /// An expression of unknown provenance, lowered afresh.
    pub fn eval_expr_for_test(&mut self, e: &Expr) -> Result<Value, Diagnostic> {
        self.drive(lower(e), Env::empty(), 0)
    }

    /// An expression from `module`, with `bindings` already in scope.
    pub fn eval_expr_in(
        &mut self,
        e: &'a Expr,
        module: usize,
        bindings: &[(Symbol, Value)],
    ) -> Result<Value, Diagnostic> {
        let mut env = Env::empty();
        for (name, value) in bindings {
            env = env.bind(name.clone(), value.clone());
        }
        let body = self.lowering.body(e);
        self.drive(body, env, module)
    }

    /// `name` is the program-wide name — `app.main`, not `main`.
    pub fn call(&mut self, name: &str, args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
        let sym = Symbol::new(name);
        let f = self.definition(&sym).ok_or_else(|| {
            Diagnostic::error(codes::UNKNOWN_NAME, format!("no definition named `{name}`"))
                .primary(span, "not defined in this program")
                .note("this name is program-wide: `store.orders.place`, not `place`")
        })?;
        // Before `reset`, so a refusal leaves the previous run's arena alone and the argument's
        // slot is still describable.
        let boundary = crate::escape::Boundary::EntryPoint { name };
        for arg in &args {
            crate::escape::check(&boundary, arg, span)?;
        }
        self.enter(f, args, span)
    }

    /// The same call **without** the entry-point boundary check, for a caller
    /// that is not an entry point.
    ///
    /// The one such caller is [`crate::backend::Reference`], which is handed a
    /// call the outer machine is in the middle of and answers it on a machine of
    /// its own. `escape::check` asks "does this argument carry a [`Value::Cell`],
    /// [`Value::Task`] or [`Value::Continuation`]", and `compiled::admit` has
    /// already answered it: every argument that reaches a backend is either
    /// childless — an `i64`, a `bool`, an `Arc<[u8]>` — or of a declared type
    /// `compiled::CarriedTypes` cleared of reaching any of those three at any
    /// depth. Asking again is not a second opinion; it is the same question
    /// asked of the value instead of the type, which is precisely the
    /// **O(value) walk per call** the front-end measurements and [`crate::census`] measured as
    /// unaffordable on a real front end and which the type gate exists to avoid.
    ///
    /// It is **not** a check being dropped for speed. The boundary this refuses
    /// at is `Boundary::EntryPoint`, and a compiled entry is not one: the
    /// machine's own inner calls — `apply` on a closure inside a body — do not
    /// run it either. What keeps a handle out of a backend is `compiled::admit`,
    /// and `a_cell_touching_caller_agrees_slot_for_slot_with_an_entered_callee`
    /// is where that is asserted end to end.
    pub(crate) fn call_within(
        &mut self,
        name: &str,
        args: Vec<Value>,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let sym = Symbol::new(name);
        let f = self.definition(&sym).ok_or_else(|| {
            Diagnostic::error(codes::UNKNOWN_NAME, format!("no definition named `{name}`"))
                .primary(span, "not defined in this program")
                .note("this name is program-wide: `store.orders.place`, not `place`")
        })?;
        self.enter(f, args, span)
    }

    /// Reset the world, run the body, and hand back whatever the run is holding.
    fn enter(&mut self, f: Value, args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
        self.reset();
        // The same three lines [`Machine::drive`] ends with, and for the same reason: this is an
        // entry point — it resets the world before it starts — so whatever a host handler is
        // holding for it has to be handed back when it ends, on the diagnostic path as much as on
        // the value path.
        let outcome = self.apply(f, args, span).and_then(|()| self.run());
        // The same close [`Machine::drive`] gives a test.
        let outcome = self.close_regions(outcome);
        self.close_open_regions();
        self.end_entry_point();
        outcome
    }

    /// One transition.
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
                // Charged after the declaration check and before the handler search, which is where
                // an unhandled `perform` was still performed, and two
                // engines that record it at different moments disagree on a failing program.
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
                    // A step's accesses are every atom the tracer recorded as well as every cell
                    // the world did.
                    if let Some(atom) = atom {
                        self.trail.record_access(Access::Atom(atom));
                    }
                }
                let answered = {
                    let Machine {
                        stack,
                        regions,
                        host_ops,
                        ..
                    } = &mut *self;
                    // A closure rather than a value: a pin is an `Rc` allocation, and `perform`
                    // only calls this for a capture that can outlive the region it was taken in.
                    let mut pin = || regions.pin();
                    handler::perform(stack, request, decl, *host_ops, &mut pin)?
                };
                match answered {
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

    /// A stack that is a value cannot leak from one entry point to the next, so this restores the
    /// world rather than unwinding anything.
    fn reset(&mut self) {
        self.stack = Stack::new();
        self.regions.reset();
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
        let outcome = self.close_regions(outcome);
        self.close_open_regions();
        self.end_entry_point();
        outcome
    }

    /// Closes every region the entry point still had open.
    fn close_open_regions(&mut self) {
        self.stack = Stack::new();
        self.sims.clear();
        self.regions.close_program_regions();
    }

    /// Hands the host runtime every exit path from an entry point.
    fn end_entry_point(&mut self) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        if let Err(diagnostic) = runtime.end_entry_point(self.id) {
            self.teardown.push(diagnostic);
        }
    }

    /// What the host runtime reported while closing entry points, and forgotten here.
    pub fn take_teardown_warnings(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.teardown)
    }

    /// A failure abandons the region where it stands.
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

    /// Adopts what a [`handler`] transition decided.
    pub(crate) fn take(&mut self, transition: Transition) -> Result<(), Diagnostic> {
        if transition.stack.calls() > self.max_calls {
            return Err(self.err_call_limit(self.current, &transition.stack));
        }
        if let Some(max) = self.max_frames
            && transition.stack.frames() > max
        {
            return Err(self.err_frame_ceiling(self.current, max, &transition.stack));
        }
        self.stack = transition.stack;
        self.state = transition.state.into();
        Ok(())
    }

    fn eval(&mut self, code: &Code, env: Env, module: usize) -> Result<(), Diagnostic> {
        let span = code.span;
        self.current = span;
        match &code.kind {
            // Built at lowering; this is a refcount bump for a `Str` or a `Bytes` and a copy of an
            // inline variant for everything else.
            NodeKind::Lit(_, value) => self.go_return(value.clone()),

            // The reference-counting pass says whether this is the last read of a binding of this
            // scope.
            NodeKind::Var(q) => {
                let mut env = env;
                let value = match (code.own, q.is_bare()) {
                    (Own::Owned, true) => match env.take_unique(q.symbol()) {
                        Some(value) => value,
                        None => self.lookup(q, &env, module)?,
                    },
                    _ => self.lookup(q, &env, module)?,
                };
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

            NodeKind::Lambda { params, body, free } => {
                let captured = match free {
                    Some(free) => env.keep_only(free),
                    None => env.clone(),
                };
                self.go_return(Value::Closure(Arc::new(Closure {
                    name: None,
                    kind: ClosureKind::Code {
                        params: params.clone(),
                        body: body.clone(),
                        env: captured,
                        module,
                    },
                })));
            }

            NodeKind::App { func, args, dead } => {
                let carried = crate::rc::carry(&env, !args.is_empty());
                self.push(
                    Frame::AppCallee {
                        args: args.clone(),
                        dead: dead.clone(),
                        env: carried,
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
                        // Unread while `next` is 0: the value this frame is waiting for *is* the
                        // scrutinee.
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

            NodeKind::Record { fields, dead } => {
                if fields.is_empty() {
                    self.go_return(Value::Record(Arc::new(Fields::default())));
                } else {
                    let carried = crate::rc::carry_released(
                        &env,
                        fields.len() > 1,
                        dead.first().map(|d| &**d).unwrap_or(&[]),
                    );
                    self.push(
                        Frame::RecordField {
                            done: Vec::with_capacity(fields.len()),
                            fields: fields.clone(),
                            dead: dead.clone(),
                            next: 1,
                            env: carried,
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
                    let carried = crate::rc::carry(&env, items.len() > 1);
                    self.push(
                        Frame::ListItem {
                            done: Vec::with_capacity(items.len()),
                            items: items.clone(),
                            next: 1,
                            env: carried,
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
                    span,
                );
                self.take(transition)?;
            }

            NodeKind::WithRegion { body } => {
                let kind = self.region_kind(span);
                let stack = self.stack.clone();
                let transition = handler::enter_with_region(
                    &mut self.regions,
                    &stack,
                    body,
                    &env,
                    module,
                    kind,
                    span,
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

        // Before resolution and before the hermetic check, because this is the terminal answer: a
        // region that reaches a socket cannot be repaired by `--host`, and a diagnostic that
        // suggests a flag which will then refuse the program has cost the reader a round trip to
        // learn nothing.
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
                    // E0424's second remedy is `--host`, and for a test the search re-runs that
                    // flag leads straight to `E0425`.
                    match self.re_executed {
                        true => hermetic.note(
                            "`--host` would then refuse this: the search runs a seeded test whole once per interleaving, so a handler would answer it once per schedule",
                        ),
                        false => hermetic,
                    }
                }
                Some(path)
                    if self
                        .binding
                        .withholds(&effect, &op, resource.as_ref())
                        .is_some() =>
                {
                    err_withheld(span, &operation, &effect, path)
                }
                Some(path) => err_unenumerated_atom(span, &operation, path),
            });
        };

        // `task.*` is answered by opening a region rather than by calling a handler: a task is a
        // suspended machine state, and a handler is handed argument values and a span.
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

        // Outside every region and still re-executed.
        if self.re_executed {
            return Err(err_host_in_search(span, &operation, declaration.path));
        }

        // The last thing checked before the handler runs, because the whole point is that the
        // credential has not crossed yet when this fires.
        if !declaration.secrets
            && let Some(position) = args.iter().position(carries_secret)
        {
            return Err(err_secret_to_host(
                span,
                &operation,
                position,
                declaration.path,
            ));
        }

        // Beside it, and for the same reason: a handler outlives every region the program opens, so
        // a handle into one has not crossed yet when this fires.
        crate::escape::check_arguments(&operation, declaration.path, &args, span)?;

        let runtime = self.runtime.clone();
        let answered = {
            let request = HostRequest {
                atom: atom.clone(),
                op: &declaration,
                args: &args,
                span,
                machine: self.id,
                task: self.performing_task(),
                declared: self.declared.as_ref(),
            };
            match &runtime {
                Some(rt) => handler.call(rt.as_ref(), &request),
                None => handler.call(&Unbound, &request),
            }
        };

        // The operation happened — a refusal included, because a handler that failed may have acted
        // before it failed and nothing here can know.
        self.host_use.record(&atom);
        if declaration.linearity.is_linear() {
            self.host_ops = self.host_ops.saturating_add(1);
            self.last_linear = Some(HostMark {
                operation: operation.clone(),
                path: declaration.path,
                span,
            });
        }

        // Every refusal is stamped with where it came from.
        let answer = answered.map_err(|d| attribute(d, declaration.path, &operation, span))?;

        let pending = match answer {
            HostAnswer::Value(value) => {
                // The declaration's structural half, and the only half of it anything can check:
                // `blocking: true` says the work left this thread and a token is coming back, so a
                // value is this thread having done the work while every task sharing it waited.
                if declaration.blocking {
                    return Err(err_blocking_answered_inline(
                        span,
                        &operation,
                        declaration.path,
                    ));
                }
                check_host_answer(&operation, declaration.path, &value, span)?;
                self.go_return(value);
                return Ok(());
            }
            HostAnswer::Pending(pending) => pending,
        };
        let Some(rt) = runtime else {
            return Err(err_no_runtime(span, &operation, pending, declaration.path));
        };

        // Inside a production region the performing task leaves the enabled set and the others keep
        // running — which is the whole of the blocking rule.
        if let Some(region) = self.host_region() {
            let Some(segments) = self.stack.sim_depth() else {
                return Err(err_task_lost_its_region(span, &operation));
            };
            let (k, _) = self.stack.capture(segments, self.host_ops);
            // Parked until the host answers, which may be after every region open here has closed.
            let k = k.pinned(self.regions.pin());
            let live = region_mut(&mut self.sims, region).expect("the region was just found");
            live.sched.park_on_host(k, pending, span)?;
            return self.schedule();
        }

        // Outside one a `Pending` has nowhere to park, so the machine drives the runtime until the
        // token resolves.
        let value = rt.block_on(pending)?;
        check_host_answer(&operation, declaration.path, &value, span)?;
        self.go_return(value);
        Ok(())
    }

    /// Who is performing, as a host handler holding scoped state has to key it.
    fn performing_task(&self) -> Option<crate::sim::TaskId> {
        let region = self.host_region()?;
        self.sims
            .iter()
            .find(|live| live.id == region)
            .and_then(|live| live.sched.current())
    }

    /// The innermost live production region, if control is actually inside it.
    fn host_region(&self) -> Option<SimId> {
        self.sims
            .iter()
            .rev()
            .find(|region| region.sched.policy() == Policy::Host && self.stack.holds_sim(region.id))
            .map(|region| region.id)
    }

    /// Opens the production region a `task.*` perform reached the binding at, or answers into one
    /// already open.
    fn open_production_region(
        &mut self,
        effect: Symbol,
        op: Symbol,
        args: Vec<Value>,
        span: Span,
    ) -> Result<(), Diagnostic> {
        // A production region already live means control left its delimiter — `find_handler` would
        // have answered otherwise — so the tasks it still holds will never run.
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
    fn innermost_simulation(&self) -> Option<Span> {
        self.sims
            .iter()
            .rev()
            .find(|region| region.sched.policy() == Policy::Seeded)
            .map(|region| region.span)
    }

    /// `E0426`, built from what [`handler::resume`] refused and the operation this machine is
    /// protecting.
    fn err_replayed(&self, refused: handler::Replayed) -> Diagnostic {
        err_continuation_resumed(self.current, refused.resumes, self.last_linear.as_ref())
    }

    /// `simulate { body }` — install the seeded scheduler and give its root task the first step.
    fn enter_simulate(
        &mut self,
        body: Code,
        env: Env,
        module: usize,
        span: Span,
    ) -> Result<(), Diagnostic> {
        if let Some(outer) = self.sims.last() {
            // Two mistakes with one symptom.
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

    /// A `task.*`, `clock.*` or `random.*` perform that reached its region's delimiter.
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
        // A `Sim` delimiter is found by operation name alone, which is right for a seeded region
        // and too wide for a production one: it answers `task` and nothing else, so a `clock.now`
        // inside it must go to the host binding rather than be given virtual time that starts at
        // zero and only moves when every task is asleep.
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
                // The delimiters the spawn site sat under, which is what the spawned body has to
                // run under: a `handle` written inside the region encloses the tasks it lexically
                // contains, and the row `spawn` publishes already says the enclosing handler
                // discharges them.
                let over = k.delimiters();
                let pin = self.regions.pin();
                let live = region_mut(&mut self.sims, region).expect("the region was just found");
                let id = live.sched.spawn(body, over, span, pin);
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
    fn record_cell_access(&mut self, b: Builtin, args: &[Value]) {
        let mode = match b {
            Builtin::CellGet => Mode::Read,
            Builtin::CellSet => Mode::Write,
            _ => return,
        };
        let Some(Value::Cell(slot)) = args.first() else {
            return;
        };
        if self.sims.is_empty() {
            return;
        }
        self.trail.record_access(Access::Cell { id: *slot, mode });
        self.note_step_site(self.current);
    }

    /// Puts a cell *allocation* into the current step's access set.
    pub(crate) fn record_alloc_access(&mut self) {
        if self.sims.is_empty() {
            return;
        }
        self.trail.record_access(Access::Alloc);
        self.note_step_site(self.current);
    }

    /// Names where the running step is standing, once per step.
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

    /// Give the next step to whichever task the seed names — or, in a production region, to
    /// whichever is ready — or deliver the region's value when every task has finished.
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
            // The runtime is reached only with nothing enabled: a region whose tasks are all
            // value-shaped never polls and never parks, so a machine bound without a reactor still
            // schedules.
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
                // A production region's clock and stream were never drawn from, so handing them to
                // the trail would report zeroes over whatever a seeded region earlier in the entry
                // point actually reached.
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

    /// Splices a continuation the program itself named — a `resume k` clause's `k`, or the implicit
    /// one a tail-resumptive clause gets.
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

    /// The stack is moved out rather than borrowed: a pop that owns its stack takes the frame out
    /// of its link instead of cloning it.
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
            // A task's body returned.
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
            // A closure this machine did not lower, handed in through `call`.
            ClosureKind::Fn {
                params,
                body,
                env,
                module,
            } => {
                let params: Vec<Symbol> = params.clone();
                let body = self.closure_code.of(&params, body);
                let (env, module) = (env.clone(), *module);
                self.enter_code(closure, &params, body, env, module, args, span)
            }
            ClosureKind::Ctor { name, arity } => {
                if crate::census::enabled() {
                    crate::census::with(|c| c.ctor_calls += 1);
                }
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
                if crate::census::enabled() {
                    let label: &'static str = b.name();
                    crate::census::with(|c| {
                        c.builtin_calls += 1;
                        *c.builtin_names.entry(label).or_default() += 1;
                    });
                }
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
        mut args: Vec<Value>,
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
        let memo = match (params.is_empty(), &closure.name) {
            (true, Some(name)) => match self.constant(name) {
                Lookup::Known(value) => {
                    self.go_return(value);
                    return Ok(());
                }
                Lookup::Remember => true,
                Lookup::Ignore => false,
            },
            _ => false,
        };
        if crate::census::enabled() {
            self.census_call(closure, &args);
        }
        if let Some(value) = self.compiled_answer(closure, &args) {
            // The interpreted path moves these into the callee's `Env`; a scalar carries no
            // refcount and no `Drop`, so dropping them here is the same observation.
            args.clear();
            crate::argv::give(args);
            // The bound was charged in `compiled_answer` — a zero budget declines — so what is left
            // is the frame bound `push` checks, in the order the interpreted path below checks
            // them.
            self.push(
                Frame::Call {
                    name: closure.name.clone(),
                    call_site: span,
                    memo,
                },
                span,
            )?;
            self.go_return(value);
            return Ok(());
        }
        let mut scope = env;
        for (p, v) in params.iter().zip(args.drain(..)) {
            scope = scope.bind(p.clone(), v);
        }
        crate::argv::give(args);
        if self.stack.calls() >= self.max_calls {
            return Err(self.err_call_limit(span, &self.stack));
        }
        self.push(
            Frame::Call {
                name: closure.name.clone(),
                call_site: span,
                memo,
            },
            span,
        )?;
        self.go_eval(body, scope, module);
        Ok(())
    }

    /// The memo is not consulted inside a `simulate` region, and nothing is written to it from one.
    fn constant(&mut self, name: &Symbol) -> Lookup {
        if !self.sims.is_empty() {
            return Lookup::Ignore;
        }
        self.memo.lookup(self.check, name)
    }

    pub(crate) fn remember_constant(&mut self, name: &Symbol, value: &Value) {
        if self.sims.is_empty() {
            self.memo.remember(name, value);
        }
    }

    /// The compiled answer for this call, or `None` to evaluate it.
    fn census_call(&self, closure: &Closure, args: &[Value]) {
        let outcome = crate::compiled::admit(
            closure,
            args,
            !self.sims.is_empty(),
            self.check,
            self.carried_types(),
            self.max_calls,
            self.stack.calls(),
        );
        let frame_ceiling = self.max_frames.is_some();
        let name = closure
            .name
            .as_ref()
            .map(|n| n.as_str().to_string())
            .unwrap_or_else(|| "<anonymous>".to_string());
        let carried_sig = match &outcome {
            Ok((n, _)) => crate::backend::carried_signature(self.carried_types(), n),
            _ => false,
        };
        // The same predicate by the other route: a walk over the declared types rather than the
        // per-definition `Denotes` the table precomputed.
        let carried_sig_walked = match (&outcome, self.check) {
            (Ok((n, _)), Some(check)) => check.defs.get(*n).is_some_and(|d| match &d.scheme.ty {
                ply_core::ty::Type::Fn { params, ret, .. } => {
                    params.iter().all(|t| self.carried_types().carries(t, None))
                        && self.carried_types().carries(ret, None)
                }
                _ => false,
            }),
            _ => false,
        };
        let blocking: Vec<&'static str> =
            if matches!(outcome, Err(crate::compiled::Gate::ArgumentShape)) {
                args.iter()
                    .filter(|v| !crate::compiled::crossable_argument_kind(v))
                    .map(crate::census::value_kind)
                    .collect()
            } else {
                Vec::new()
            };
        let blocking_type: Option<&'static str> =
            matches!(outcome, Err(crate::compiled::Gate::ArgumentType)).then(|| {
                let name = closure.name.as_ref().expect("the name gate ran first");
                self.carried_types().refusal(self.check, name, args)
            });
        let ladder: Vec<(&'static str, bool, bool)> = crate::census::LADDER
            .iter()
            .map(|(label, kinds, deep)| {
                let ok = crate::compiled::admit_with(
                    closure,
                    args,
                    !self.sims.is_empty(),
                    self.check,
                    None,
                    self.max_calls,
                    self.stack.calls(),
                    |v| {
                        if *deep {
                            crate::census::kind_in_deep(v, kinds)
                        } else {
                            crate::census::kind_in(v, kinds)
                        }
                    },
                );
                let returnable = match (&ok, self.check) {
                    (Ok((n, _)), Some(check)) => check.defs.get(*n).is_some_and(|d| {
                        matches!(&d.scheme.ty, ply_core::ty::Type::Fn { ret, .. }
                            if crate::census::type_carries(ret, kinds))
                    }),
                    _ => false,
                };
                (*label, ok.is_ok(), returnable)
            })
            .collect();
        // The widest rung, attributed: only for calls the shallow twin carries and the deep one
        // does not, so this counts the gap and nothing else.
        let (widest_label, widest_kinds, _) =
            crate::census::LADDER[crate::census::LADDER.len() - 1];
        let deep_blocker = (ladder
            .iter()
            .any(|(l, ok, _)| *l == "4 no world-handle  shallow" && *ok)
            && !ladder.iter().any(|(l, ok, _)| *l == widest_label && *ok))
        .then(|| {
            args.iter()
                .find_map(|v| crate::census::deep_blocker(v, widest_kinds))
                .unwrap_or("<none>")
        });
        // The type-level alternative to a deep walk: every gate but the shape one, and the
        // arguments decided from the declared parameter types.
        let (_, widest_kinds_t, _) = crate::census::LADDER[crate::census::LADDER.len() - 1];
        let (type_gated, type_gated_and_return) = match (
            crate::compiled::admit_with(
                closure,
                args,
                !self.sims.is_empty(),
                self.check,
                None,
                self.max_calls,
                self.stack.calls(),
                |_| true,
            ),
            self.check,
        ) {
            (Ok((n, _)), Some(check)) => match check.defs.get(n).map(|d| &d.scheme.ty) {
                Some(ply_core::ty::Type::Fn { params, ret, .. }) => {
                    let ps = params
                        .iter()
                        .all(|t| crate::census::type_carries(t, widest_kinds_t));
                    (ps, ps && crate::census::type_carries(ret, widest_kinds_t))
                }
                _ => (false, false),
            },
            _ => (false, false),
        };

        // The SHIPPING type gate, asked with the value-kind test removed.
        let type_gated_shipping = crate::compiled::admit_with(
            closure,
            args,
            !self.sims.is_empty(),
            self.check,
            Some(self.carried_types()),
            self.max_calls,
            self.stack.calls(),
            |_| true,
        )
        .is_ok();

        crate::census::with(|c| {
            c.body_calls += 1;
            if type_gated {
                c.type_gated += 1;
            }
            if type_gated_shipping {
                c.type_gated_shipping += 1;
            }
            if type_gated_and_return {
                c.type_gated_and_return += 1;
            }
            if let Some(k) = deep_blocker {
                *c.deep_blockers.entry(k).or_default() += 1;
            }
            for (label, ok, returnable) in &ladder {
                if *ok {
                    *c.widened.entry(label).or_default() += 1;
                    if *returnable {
                        *c.widened_returnable.entry(label).or_default() += 1;
                    }
                }
            }
            if frame_ceiling {
                c.frame_ceiling += 1;
            }
            match &outcome {
                Ok(_) => {
                    c.admitted += 1;
                    if carried_sig {
                        c.admitted_carried_sig += 1;
                    }
                    if carried_sig_walked {
                        c.carried_sig_walked += 1;
                    }
                    *c.admitted_names.entry(name).or_default() += 1;
                }
                Err(gate) => {
                    let g = crate::census::gate_name(*gate);
                    *c.gates.entry(g).or_default() += 1;
                    *c.refused_names.entry(format!("{name} @ {g}")).or_default() += 1;
                    for k in &blocking {
                        *c.blocking_args.entry(k).or_default() += 1;
                    }
                    if let Some(k) = blocking_type {
                        *c.blocking_types.entry(k).or_default() += 1;
                    }
                }
            }
        });
    }

    /// `Gate::ArgumentType`'s table, built on first need and then kept.
    fn carried_types(&self) -> &crate::compiled::CarriedTypes {
        self.carried_types
            .get_or_init(|| crate::compiled::CarriedTypes::over(self.check))
    }

    fn compiled_answer(&self, closure: &Closure, args: &[Value]) -> Option<Value> {
        let backend = self.compiled.as_ref()?;
        // A native body pends no frames, so it cannot honour a ceiling counted in them — `enter` is
        // handed the call budget and there is nothing else to hand it.
        if self.max_frames.is_some() {
            return None;
        }
        let (name, budget) = crate::compiled::admit(
            closure,
            args,
            !self.sims.is_empty(),
            self.check,
            self.carried_types(),
            self.max_calls,
            self.stack.calls(),
        )
        .ok()?;

        #[cfg(debug_assertions)]
        let before = self.compiled_witness();
        let answer = backend.enter(name, args, budget);
        #[cfg(debug_assertions)]
        debug_assert!(
            before == self.compiled_witness(),
            "a compiled body moved machine state it was handed no route to"
        );

        match answer {
            // The answer test, and it is `admit`'s argument test asked once more at the other end:
            // the declared RETURN type is carried and the answer is of the kind it denotes, or the
            // answer is childless and the old `crossable` rule carries it unchanged.
            Some(value) if self.carried_types().answer_crosses(name, &value) => {
                self.compiled_entries.set(self.compiled_entries.get() + 1);
                Some(value)
            }
            // Refused in every profile, on purpose.
            Some(_) => {
                self.compiled_refusals.set(self.compiled_refusals.get() + 1);
                self.compiled_declines.set(self.compiled_declines.get() + 1);
                None
            }
            None => {
                self.compiled_declines.set(self.compiled_declines.get() + 1);
                None
            }
        }
    }

    /// What a compiled body is handed no route to move.
    #[cfg(debug_assertions)]
    fn compiled_witness(&self) -> (usize, usize, u64, usize, u64, u64) {
        let arena = self.regions.arena().stats();
        (
            self.stack.frames(),
            self.stack.calls(),
            self.host_ops,
            self.sims.len(),
            arena.allocations,
            arena.regions_opened,
        )
    }

    pub(crate) fn call_builtin(
        &mut self,
        b: Builtin,
        args: Vec<Value>,
        span: Span,
    ) -> Result<(), Diagnostic> {
        self.record_cell_access(b, &args);
        let step = builtins::call(b, args, self.regions.arena_mut(), span)?;
        self.run_builtin_step(step, span)
    }

    /// The machine's half of the builtin step protocol: a suspension becomes a frame on the heap,
    /// so a continuation captured inside `map`'s callback can be resumed as many times as it likes.
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
            let code = stmt.code().clone();
            // The continuation carries only what it still reads.
            let rest = scope.release(stmt.dead());
            let span = code.span;
            self.push(
                Frame::BlockStep {
                    stmts,
                    next: next + 1,
                    tail,
                    scope: rest,
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
                // A nullary constructor written bare is indistinguishable from a binder in the AST,
                // so the constructor table decides.
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
            PatternKind::Lit(lit) => crate::semantics::lit_matches(lit, value),
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

    /// Locals, then the module's own items and its selective imports, then the prelude — the
    /// resolution order the whole language is specified in.
    fn lookup(&mut self, q: &QName, env: &Env, module: usize) -> Result<Value, Diagnostic> {
        if q.is_bare() {
            match env.lookup(q.symbol()) {
                Some(Slot::Live(v)) => return Ok(v.clone()),
                // The reference-counting pass called this binding dead and it was read anyway.
                Some(Slot::Released) => return Err(err_released(q, self.current)),
                None => {}
            }
        }
        let key = (
            module,
            q.module.as_ref().map(|m| m.name.clone()),
            q.name.name.clone(),
        );
        if let Some(v) = self.globals.get(&key) {
            return Ok(v.clone());
        }
        if let Some(name) = self.global(module, Namespace::Value, q)
            && let Some(v) = self.definition(&name)
        {
            self.globals.insert(key, v.clone());
            return Ok(v);
        }
        if let Some(name) = self.ctor_name(module, q)
            && let Some(&arity) = self.ctors.get(&name)
        {
            let v = ctor_value(&name, arity);
            self.globals.insert(key, v.clone());
            return Ok(v);
        }
        if q.is_bare()
            && let Some(b) = Builtin::from_name(q.symbol())
        {
            let v = Value::builtin(b);
            self.globals.insert(key, v.clone());
            return Ok(v);
        }
        Err(err_unknown_name(q))
    }

    /// The closure for a program-wide name, lowering its body the first time **any** machine over
    /// this program reaches it.
    fn definition(&mut self, name: &Symbol) -> Option<Value> {
        if let Some(v) = self.lowered.get(name) {
            return Some(v.clone());
        }
        let slot = self.fns.get(name)?;
        let (def, module) = (slot.def, slot.module);
        let params: code::Params =
            Rc::new(def.params.iter().map(|p| p.name.name.clone()).collect());
        let body = self.lowering.of(&params, &def.body);
        let closure = Closure {
            name: Some(name.clone()),
            kind: ClosureKind::Code {
                body,
                params,
                env: Env::empty(),
                module,
            },
        };
        let value = Value::Closure(Arc::new(closure));
        self.lowered.insert(name.clone(), value.clone());
        Some(value)
    }

    /// Resolution already decided what this denotes; nothing here re-derives it.
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

    /// An effect no module declares keeps the name as written.
    fn effect_name(&self, module: usize, effect: &QName) -> Symbol {
        self.global(module, Namespace::Effect, effect)
            .unwrap_or_else(|| effect.symbol().clone())
    }

    pub(crate) fn push(&mut self, frame: Frame, span: Span) -> Result<(), Diagnostic> {
        if let Some(max) = self.max_frames
            && self.stack.frames() >= max
        {
            return Err(self.err_frame_ceiling(span, max, &self.stack));
        }
        self.stack = std::mem::take(&mut self.stack).pushed(frame);
        Ok(())
    }

    pub(crate) fn regions_mut(&mut self) -> &mut TaskRegions {
        &mut self.regions
    }

    /// The semantic bound.
    fn err_call_limit(&self, span: Span, stack: &Stack) -> Diagnostic {
        limit::err_recursion_limit(span, NESTED_CALLS, self.max_calls, &innermost_calls(stack))
    }

    /// Only reachable through [`Machine::with_max_frames`], and phrased so a reader can tell it
    /// apart from the semantic bound: this one says what ran out of what, and
    /// never the words "recursion limit".
    fn err_frame_ceiling(&self, span: Span, max: usize, stack: &Stack) -> Diagnostic {
        limit::err_frame_ceiling(span, max, self.max_calls, &innermost_calls(stack))
    }
}

/// The live region with this id, if it is the one still running.
fn region_mut(sims: &mut [Region], id: SimId) -> Option<&mut Region> {
    sims.iter_mut().find(|region| region.id == id)
}

/// Puts a spawned task under the delimiters its `spawn` site sat under.
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

/// A task about to park on a host token whose region delimiter is not on its stack.
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

/// A binding the reference-counting pass dropped, read afterwards.
#[cold]
#[inline(never)]
fn err_released(q: &QName, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{q}` was read after reference counting dropped it"),
    )
    .primary(span, "this binding was released before this read")
    .note("the last-use analysis in `ply_eval::rc` called this binding dead and something read it anyway")
    .note("reaching this is a defect in Ply, not in the program: please report it with the definition that produced it")
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

/// `E0427` — a registration claims this operation, the run is bound, and the binding enumerated no
/// atom for it.
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

/// Whether a value handed across the boundary holds a credential anywhere.
pub(crate) fn check_host_answer(
    operation: &str,
    path: &'static str,
    value: &Value,
    span: Span,
) -> Result<(), Diagnostic> {
    crate::escape::check(
        &crate::escape::Boundary::HostAnswer { operation, path },
        value,
        span,
    )
}

fn carries_secret(v: &Value) -> bool {
    match v {
        Value::Secret(_) => true,
        Value::List(xs) => crate::limit::grow(|| xs.iter().any(carries_secret)),
        Value::Map(m) => crate::limit::grow(|| {
            m.iter()
                .any(|(k, v)| carries_secret(k) || carries_secret(v))
        }),
        Value::Record(fields) => crate::limit::grow(|| fields.values().any(carries_secret)),
        Value::Ctor { args, .. } => crate::limit::grow(|| args.iter().any(carries_secret)),
        _ => false,
    }
}

/// `E0439` — a credential reached a host operation whose registration does not declare that it may
/// receive one.
#[cold]
#[inline(never)]
fn err_secret_to_host(
    span: Span,
    operation: &str,
    position: usize,
    path: &'static str,
) -> Diagnostic {
    Diagnostic::error(
        codes::SECRET_TO_HOST,
        format!("`{operation}` was handed a `Secret` in argument {}", position + 1),
    )
    .primary(span, "performed here")
    .note(format!("`{path}` is registered `secrets: no`, so nothing above the boundary knows a credential can reach it"))
    .note("below the boundary nothing is checkable: what a handler does with a credential is invisible to every guarantee this language makes")
    .note("`ply hosts` prints the column; a handler that must receive one declares `secrets: true` there and becomes a reviewed member of the trusted computing base")
    .note("this is Ply's fault: the registration and what crossed it disagree, and no definition in the program decides which was meant")
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

/// A scheduled operation whose region is gone: a `Task` or a continuation smuggled past the `}`
/// that ended the scheduler it names.
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

/// The innermost pending calls, innermost first.
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
        // `-0.0` is a `Float` distinct from `0.0` and negation is how a program reaches it, so this
        // arm is not decoration.
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
        // Every bit of the two's-complement pattern flipped, so `~0` is `-1`.
        UnOp::BitNot => Ok(Value::Int(!value.as_int(operand_span, "`~`")?)),
    }
}

/// `||` is decided by a `true` left operand and `&&` by a `false` one; anything else has to
/// evaluate the right.
pub(crate) fn short_circuits(op: BinOp, lhs: bool) -> bool {
    lhs == matches!(op, BinOp::Or)
}

#[cfg(test)]
mod tests;

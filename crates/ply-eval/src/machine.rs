//! The control-stack machine.

use crate::arena::Arena;
use crate::arena::RegionKind;
use crate::builtins::{self, Builtin, Step};
use crate::code::{
    self, Captures, ClosureCode, Code, Lowered, Lowering, NodeKind, Pat, Stmt as CodeStmt, lower,
};
use crate::cont::{Continuation, Delimiter, Extent, Frame, Next, Prompt, SimId, Stack};
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
use crate::window::{SlotVal, Windows};
use ply_core::CheckOutput;
use ply_core::ty::{EffectAtom, Footprint};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{BinOp, Expr, FnDef, Item, Mode, Program, QName, TypeDefBody, UnOp};
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
    Eval { code: Code, module: usize },
    Return(Value),
    Perform(Request),
    Halt(Value),
}

impl From<handler::State> for State {
    fn from(s: handler::State) -> State {
        match s {
            handler::State::Eval { code, module } => State::Eval { code, module },
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
    /// The machine-owned slot stack, and the current activation's base.
    windows: Windows,
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
            windows: Windows::new(),
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
        self.drive(body, module).map(|_| ())
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
        self.drive(body, owner).map(|_| ())
    }

    /// An expression of unknown provenance, lowered afresh.
    pub fn eval_expr_for_test(&mut self, e: &Expr) -> Result<Value, Diagnostic> {
        self.drive(lower(e), 0)
    }

    /// An expression from `module`, with `bindings` already in scope: the names are lowered as
    /// leading parameters of the body's window, so their occurrences resolve to slots exactly as
    /// a function's parameters do.
    pub fn eval_expr_in(
        &mut self,
        e: &'a Expr,
        module: usize,
        bindings: &[(Symbol, Value)],
    ) -> Result<Value, Diagnostic> {
        let params: code::Params = Rc::new(bindings.iter().map(|(n, _)| n.clone()).collect());
        let lowered = self.lowering.of(&params, e);
        let values: Vec<Value> = bindings.iter().map(|(_, v)| v.clone()).collect();
        self.drive_with(lowered, module, &values)
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
            State::Eval { code, module } => {
                self.eval(&code, module)?;
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
                        windows,
                        regions,
                        host_ops,
                        ..
                    } = &mut *self;
                    // A closure rather than a value: a pin is an `Rc` allocation, and `perform`
                    // only calls this for a capture that can outlive the region it was taken in.
                    let mut pin = || regions.pin();
                    handler::perform(stack, windows, request, decl, *host_ops, &mut pin)?
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
        self.windows.clear();
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

    fn drive(&mut self, body: Lowered, module: usize) -> Result<Value, Diagnostic> {
        self.drive_with(body, module, &[])
    }

    /// [`Machine::drive`], with `bindings` written into the body's leading slots.
    fn drive_with(
        &mut self,
        body: Lowered,
        module: usize,
        bindings: &[Value],
    ) -> Result<Value, Diagnostic> {
        self.reset();
        // The root window still needs an accounting frame: an entry point's stack can become a
        // task (`into_task` converts the base segment), and a capture's height walk then crosses
        // this window.
        self.stack = self.stack.push(Frame::Exit {
            callee_window: body.size,
            caller_window: 0,
        });
        let base = self.windows.enter(body.size);
        self.windows.base = base;
        for (i, v) in bindings.iter().enumerate() {
            self.windows.write(i as u32, v.clone());
        }
        self.state = State::Eval {
            code: body.code,
            module,
        };
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

    pub(crate) fn go_eval(&mut self, code: Code, module: usize) {
        self.state = State::Eval { code, module };
    }

    pub(crate) fn windows_mut(&mut self) -> &mut Windows {
        &mut self.windows
    }

    pub(crate) fn regions_and_windows(&mut self) -> (&mut TaskRegions, &mut Windows) {
        (&mut self.regions, &mut self.windows)
    }

    /// Undoes one activation window: drops the callee's slots and re-derives the caller's base
    /// from its window size — both relative, which is what lets a spliced extent's frames run at
    /// any height.
    pub(crate) fn exit_window(&mut self, callee_window: u32, caller_window: u32) {
        let to = self.windows.len() - callee_window as usize;
        self.windows.truncate(to);
        self.windows.base = to - caller_window as usize;
    }

    /// Splices a captured continuation onto the current control, restoring its slot snapshot if
    /// it carries one.
    pub(crate) fn splice(&mut self, k: &Continuation, value: Value) -> Result<(), Diagnostic> {
        k.admit(self.host_ops)
            .map_err(|resumes| self.err_replayed(handler::Replayed { resumes }))?;
        match k.extent() {
            Extent::InPlace => {
                self.stack = self.stack.resume(k);
            }
            Extent::Saved { slots } => {
                let base_offset = self.windows.window();
                self.stack = self
                    .stack
                    .push(Frame::Restore {
                        spill: k.cut_window() as u32,
                        base_offset,
                    })
                    .resume(k);
                self.windows.restore(slots);
            }
        }
        self.windows.base = self.windows.len() - k.base_offset();
        self.go_return(value);
        Ok(())
    }

    /// Undoes a seal whose continuation turned out not to be consumed — a scheduled-looking
    /// operation the region's policy does not answer, handed to the host with the machine's stack
    /// intact. The seal cloned the shared portion and drained the extent above it, so putting the
    /// drained portion back restores the exact heights.
    fn unseal(&mut self, k: &Continuation) {
        if let Extent::Saved { slots } = k.extent() {
            self.windows.restore(&slots[k.cut_window()..]);
            self.windows.base = self.windows.len() - k.base_offset();
        }
    }

    /// Copies a barrier's free-variable values out of the current window, by each capture's own
    /// ownership: a capture that is the binding's last use moves it, anything else clones. A
    /// vacant source slot is a name no binder ran for — a nullary constructor pattern — and falls
    /// back to global resolution exactly as a read would.
    fn capture_values(
        &mut self,
        captures: &Rc<Captures>,
        module: usize,
    ) -> Result<Rc<[Value]>, Diagnostic> {
        if captures.is_empty() {
            return Ok(Rc::from(Vec::new()));
        }
        let mut out: Vec<Value> = Vec::with_capacity(captures.len());
        for j in 0..captures.len() {
            let src = captures.src[j];
            enum Peek {
                Val(Value),
                Moved,
                Vacant,
            }
            let peek = match captures.owns[j] {
                Own::Owned => match self.windows.take(src) {
                    SlotVal::Full(v) => {
                        crate::rc::note_take(true);
                        Peek::Val(v)
                    }
                    SlotVal::Moved => Peek::Moved,
                    SlotVal::Vacant => Peek::Vacant,
                },
                _ => match self.windows.read(src) {
                    SlotVal::Full(v) => Peek::Val(v.clone()),
                    SlotVal::Moved => Peek::Moved,
                    SlotVal::Vacant => Peek::Vacant,
                },
            };
            out.push(match peek {
                Peek::Val(v) => v,
                Peek::Moved => return Err(err_released(&captures.names[j], self.current)),
                Peek::Vacant => self.lookup_name(&captures.names[j], module)?,
            });
        }
        Ok(Rc::from(out))
    }

    /// Global resolution of a bare name, for a read that fell through a vacant slot.
    fn lookup_name(&mut self, name: &Symbol, module: usize) -> Result<Value, Diagnostic> {
        let q = QName::bare(ply_syntax::ast::Ident::new(name.as_str(), self.current));
        self.lookup(&q, module)
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

    fn eval(&mut self, code: &Code, module: usize) -> Result<(), Diagnostic> {
        let span = code.span;
        self.current = span;
        match &code.kind {
            // Built at lowering; this is a refcount bump for a `Str` or a `Bytes` and a copy of an
            // inline variant for everything else.
            NodeKind::Lit(_, value) => self.go_return(value.clone()),

            // The forward pass gave this occurrence a slot; the backward pass says whether it is
            // the binding's last use, which moves the value out of the slot instead of cloning.
            NodeKind::Var { name, slot } => {
                enum Peek {
                    Val(Value),
                    Moved,
                    Vacant,
                }
                let peek = match slot {
                    Some(s) => match code.own {
                        Own::Owned => match self.windows.take(*s) {
                            SlotVal::Full(v) => {
                                crate::rc::note_take(true);
                                Peek::Val(v)
                            }
                            SlotVal::Moved => Peek::Moved,
                            SlotVal::Vacant => Peek::Vacant,
                        },
                        _ => match self.windows.read(*s) {
                            SlotVal::Full(v) => Peek::Val(v.clone()),
                            SlotVal::Moved => Peek::Moved,
                            SlotVal::Vacant => Peek::Vacant,
                        },
                    },
                    None => Peek::Vacant,
                };
                let value = match peek {
                    Peek::Val(v) => v,
                    // The liveness analysis called this binding dead and it was read anyway.
                    Peek::Moved => return Err(err_released(name, span)),
                    Peek::Vacant => self.lookup(name, module)?,
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
                self.go_eval(operand.clone(), module);
            }

            NodeKind::Binary { op, lhs, rhs } => {
                self.push(
                    Frame::BinaryRhs {
                        op: *op,
                        rhs: rhs.clone(),
                        module,
                        lhs_span: lhs.span,
                        span,
                    },
                    span,
                )?;
                self.go_eval(lhs.clone(), module);
            }

            NodeKind::Lambda {
                params,
                body,
                size,
                captures,
            } => {
                let captured = self.capture_values(captures, module)?;
                self.go_return(Value::Closure(Arc::new(Closure {
                    name: None,
                    kind: ClosureKind::Code {
                        params: params.clone(),
                        body: body.clone(),
                        size: *size,
                        captures: captures.clone(),
                        captured,
                        module,
                    },
                })));
            }

            NodeKind::App { func, args } => {
                crate::rc::note_carry();
                self.push(
                    Frame::AppCallee {
                        args: args.clone(),
                        module,
                        span,
                    },
                    span,
                )?;
                self.go_eval(func.clone(), module);
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
                        module,
                        cond_span: cond.span,
                    },
                    span,
                )?;
                self.go_eval(cond.clone(), module);
            }

            NodeKind::Match { scrutinee, arms } => {
                self.push(
                    Frame::MatchArms {
                        // Unread while `next` is 0: the value this frame is waiting for *is* the
                        // scrutinee.
                        scrutinee: Value::Unit,
                        arms: arms.clone(),
                        next: 0,
                        module,
                        scrutinee_span: scrutinee.span,
                    },
                    span,
                )?;
                self.go_eval(scrutinee.clone(), module);
            }

            NodeKind::Block { stmts, tail } => {
                self.enter_block(stmts.clone(), 0, tail.clone(), module)?;
            }

            NodeKind::Record { fields } => {
                if fields.is_empty() {
                    self.go_return(Value::Record(Arc::new(Fields::default())));
                } else {
                    crate::rc::note_carry();
                    self.push(
                        Frame::RecordField {
                            done: Vec::with_capacity(fields.len()),
                            fields: fields.clone(),
                            next: 1,
                            module,
                        },
                        span,
                    )?;
                    self.go_eval(fields[0].1.clone(), module);
                }
            }

            NodeKind::Field { base, field } => {
                // Field-granular liveness: the last use of *this field* of a slot binding takes
                // it out of the record in place — when the record is unshared — while the record
                // stays put for the other fields still read later. The runtime uniqueness probe
                // keeps this sound under multi-shot resumption: a captured window's snapshot
                // holds a second reference, so the take degrades to a clone.
                if code.own == Own::OwnedField
                    && let NodeKind::Var {
                        name,
                        slot: Some(s),
                    } = &base.kind
                {
                    enum Got {
                        Val(Value),
                        Fall,
                        NotRecord(&'static str, String),
                        Moved,
                    }
                    let got = match self.windows.read_mut(*s) {
                        SlotVal::Full(Value::Record(fields)) => match Arc::get_mut(fields) {
                            Some(map) => match map.set(&field.name, Value::Unit) {
                                Some(v) => Got::Val(v),
                                None => Got::Fall,
                            },
                            None => match fields.get(&field.name) {
                                Some(v) => Got::Val(v.clone()),
                                None => Got::Fall,
                            },
                        },
                        SlotVal::Full(other) => Got::NotRecord(other.type_name(), other.render()),
                        SlotVal::Moved => Got::Moved,
                        SlotVal::Vacant => Got::Fall,
                    };
                    match got {
                        Got::Val(v) => {
                            self.go_return(v);
                            return Ok(());
                        }
                        Got::Moved => return Err(err_released(name, span)),
                        Got::NotRecord(ty, rendered) => {
                            return Err(Diagnostic::error(
                                codes::RUNTIME_ERROR,
                                format!("field access expects a record, but got {ty}"),
                            )
                            .primary(base.span, format!("this is {rendered}")));
                        }
                        // A missing field or a vacant slot reports through the ordinary path.
                        Got::Fall => {}
                    }
                }
                self.push(
                    Frame::FieldAccess {
                        field: field.clone(),
                        base_span: base.span,
                    },
                    span,
                )?;
                self.go_eval(base.clone(), module);
            }

            NodeKind::List { items } => {
                if items.is_empty() {
                    self.go_return(Value::list(Vec::new()));
                } else {
                    crate::rc::note_carry();
                    self.push(
                        Frame::ListItem {
                            done: Vec::with_capacity(items.len()),
                            items: items.clone(),
                            next: 1,
                            module,
                        },
                        span,
                    )?;
                    self.go_eval(items[0].clone(), module);
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
                // The clause bodies' free variables are copied out of the current window now, at
                // handle entry: a clause runs below its own prompt and cannot reach this
                // activation's slots when a perform arrives.
                let mut clause_captures = Vec::with_capacity(clauses.len());
                for clause in clauses.iter() {
                    clause_captures.push(self.capture_values(&clause.captures, module)?);
                }
                let ret_captures = match ret {
                    Some(arm) => self.capture_values(&arm.captures, module)?,
                    None => Rc::from(Vec::new()),
                };
                let prompt = Rc::new(Prompt {
                    clauses: clauses.clone(),
                    effects,
                    ret: ret.clone(),
                    clause_captures,
                    ret_captures,
                    module,
                    span,
                });
                let transition =
                    handler::enter_handle(&self.stack, body, prompt, module, self.windows.window());
                self.take(transition)?;
            }

            NodeKind::WithCell {
                resource,
                init,
                binder,
                slot,
                body,
            } => {
                let transition = handler::enter_with_cell(
                    &self.stack,
                    resource,
                    binder,
                    *slot,
                    init,
                    body,
                    module,
                    span,
                );
                self.take(transition)?;
            }

            NodeKind::WithRegion { body } => {
                let kind = self.region_kind(span);
                let stack = self.stack.clone();
                let transition =
                    handler::enter_with_region(&mut self.regions, &stack, body, module, kind, span);
                self.take(transition)?;
            }

            NodeKind::Simulate {
                body,
                size,
                captures,
            } => {
                self.enter_simulate(body.clone(), *size, captures.clone(), module, span)?;
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
            // Parked until the host answers, which may be after every region open here has
            // closed — with its windows sealed on, because the machine's slots keep moving while
            // it waits.
            let k = handler::seal(k, &mut self.windows).pinned(self.regions.pin());
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
        let base_offset = self.windows.window();
        let saved = self.windows.drain_all();
        let k = std::mem::take(&mut self.stack)
            .into_task(id, self.host_ops)
            .with_extent(
                Extent::Saved {
                    slots: Rc::new(saved),
                },
                base_offset,
            );
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
        size: u32,
        captures: Rc<Captures>,
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
        // The body is a barrier: what it reads from this scope it takes as a copy now, because
        // its tasks run over their own windows above the region's floor.
        let captured = self.capture_values(&captures, module)?;
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
            size,
            captures,
            captured,
            module,
            span,
            self.windows.len(),
            self.windows.base,
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
            // The capture is not consumed: the control was never cut from the machine's stack,
            // so the windows the seal drained go back exactly where they were.
            self.unseal(&k);
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
            Builtin::CellSet | Builtin::CellUpdate => Mode::Write,
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
                self.windows.truncate(region.floor);
                self.windows.base = region.rbase;
                self.stack = region.below;
                self.go_return(value);
                Ok(())
            }
            Turn::Run { resumption, .. } => {
                let (below, id) = (region.below.clone(), region.id);
                let (floor, rbase) = (region.floor, region.rbase);
                // Every turn starts from the region's entry height: the previous task's windows
                // died with its turn, captured onto its continuation or finished with it.
                self.windows.truncate(floor);
                self.windows.base = rbase;
                match resumption {
                    Resumption::Enter => {
                        let Some(body) = region.body.clone() else {
                            return Err(err_lazy_region_entered(region.span));
                        };
                        let (size, captures, captured, module) = (
                            region.size,
                            region.captures.clone(),
                            region.captured.clone(),
                            region.module,
                        );
                        // The body's window needs a frame to account for it, both so a capture's
                        // height walk sees it and so the region's own value pops it on the way
                        // out.
                        self.stack = below.push_sim(id).pushed(Frame::Exit {
                            callee_window: size,
                            caller_window: 0,
                        });
                        let base = self.windows.enter(size);
                        self.windows.base = base;
                        for (j, dst) in captures.dst.iter().enumerate() {
                            self.windows.write(*dst, captured[j].clone());
                        }
                        self.go_eval(body, module);
                        Ok(())
                    }
                    Resumption::Start { body, over, span } => {
                        self.stack = install(below, &over);
                        self.apply(body, Vec::new(), span)
                    }
                    Resumption::Resume { k, value } => {
                        self.stack = below;
                        self.splice(&k, value)
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
            if region_mut(&mut self.sims, id).is_none() || k.under_sim(&self.stack).is_none() {
                return Err(err_region_ended(self.current));
            }
            k.admit(self.host_ops)
                .map_err(|resumes| self.err_replayed(handler::Replayed { resumes }))?;
            // The `Restore` bookkeeping goes onto the stack *before* the anchor is cut, because
            // the region's completion replaces the stack with the anchor and returns through it —
            // a restore frame outside the anchor would be bypassed, leaving the restored windows
            // on the stack and the resumer's base wrong.
            if let Extent::Saved { .. } = k.extent() {
                self.stack = self.stack.push(Frame::Restore {
                    spill: k.cut_window() as u32,
                    base_offset: self.windows.window(),
                });
            }
            let anchor = k.under_sim(&self.stack).expect("checked above");
            let region = region_mut(&mut self.sims, id).expect("checked above");
            region.below = anchor;
            // The region continues wherever it is being resumed, so its floor — the height every
            // scheduling turn resets to — moves with it: the point where the `Sim` delimiter
            // lands once the extent is restored. A tail-resumptive capture left the extent in
            // place, so its floor has not moved.
            if let Extent::Saved { slots } = k.extent() {
                let above = k
                    .deltas_through_sim()
                    .expect("the continuation carries the delimiter it was anchored by");
                // The entering activation's window size survives every re-anchoring, and is what
                // re-derives its base under the delimiter's new position.
                let entering = region.floor - region.rbase;
                region.floor = self.windows.len() + slots.len() - above;
                region.rbase = region.floor - entering;
            }
            match k.extent() {
                Extent::InPlace => {
                    self.stack = self.stack.resume(k);
                }
                Extent::Saved { slots } => {
                    self.stack = self.stack.resume(k);
                    self.windows.restore(slots);
                }
            }
            self.windows.base = self.windows.len() - k.base_offset();
            self.go_return(value);
            return Ok(());
        }
        self.splice(k, value)
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
                match &prompt.ret {
                    Some(arm) => {
                        // The `return` arm is a barrier of its own: open its window above the
                        // leaving activation, fill in its captures, and bind the value to its one
                        // parameter.
                        let caller_window = self.windows.window();
                        self.stack = rest.pushed(Frame::Exit {
                            callee_window: arm.size,
                            caller_window,
                        });
                        let base = self.windows.enter(arm.size);
                        self.windows.base = base;
                        for (j, dst) in arm.captures.dst.iter().enumerate() {
                            self.windows.write(*dst, prompt.ret_captures[j].clone());
                        }
                        self.windows.write(0, value);
                        self.go_eval(arm.body.clone(), prompt.module);
                    }
                    None => {
                        self.stack = rest;
                        self.go_return(value);
                    }
                }
                Ok(())
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
                size,
                captures,
                captured,
                module,
            } => {
                let lowered = Lowered {
                    code: body.clone(),
                    size: *size,
                };
                let (captures, captured, module) = (captures.clone(), captured.clone(), *module);
                self.enter_code(
                    closure,
                    params.len(),
                    lowered,
                    Some((captures, captured)),
                    &[],
                    module,
                    args,
                    span,
                )
            }
            // A closure this machine did not lower, handed in through `call`. Its external
            // bindings are lowered as leading parameters, so the body's reads of them resolve to
            // slots like anything else.
            ClosureKind::Fn {
                params,
                body,
                bindings,
                module,
            } => {
                let pre: Vec<Symbol> = bindings.iter().map(|(n, _)| n.clone()).collect();
                let lowered = self.closure_code.of(&pre, params, body);
                let pre_values: Vec<Value> = bindings.iter().map(|(_, v)| v.clone()).collect();
                let (arity, module) = (params.len(), *module);
                self.enter_code(
                    closure,
                    arity,
                    lowered,
                    None,
                    &pre_values,
                    module,
                    args,
                    span,
                )
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
        arity: usize,
        body: Lowered,
        captures: Option<(Rc<Captures>, Rc<[Value]>)>,
        pre: &[Value],
        module: usize,
        mut args: Vec<Value>,
        span: Span,
    ) -> Result<(), Diagnostic> {
        if arity != args.len() {
            return Err(arity_error(span, &closure.describe(), arity, args.len()));
        }
        let memo = match (arity == 0 && pre.is_empty(), &closure.name) {
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
            // The interpreted path moves these into the callee's window; a scalar carries no
            // refcount and no `Drop`, so dropping them here is the same observation.
            args.clear();
            crate::argv::give(args);
            // The bound was charged in `compiled_answer` — a zero budget declines — so what is left
            // is the frame bound `push` checks, in the order the interpreted path below checks
            // them. No window was opened for a native body, so there is nothing to undo on return.
            self.push(
                Frame::Call {
                    name: closure.name.clone(),
                    call_site: span,
                    memo,
                    callee_window: 0,
                    caller_window: self.windows.window(),
                },
                span,
            )?;
            self.go_return(value);
            return Ok(());
        }
        if self.stack.calls() >= self.max_calls {
            return Err(self.err_call_limit(span, &self.stack));
        }
        self.push(
            Frame::Call {
                name: closure.name.clone(),
                call_site: span,
                memo,
                callee_window: body.size,
                caller_window: self.windows.window(),
            },
            span,
        )?;
        let base = self.windows.enter(body.size);
        self.windows.base = base;
        let mut at = 0u32;
        for v in pre.iter().cloned() {
            self.windows.write(at, v);
            at += 1;
        }
        for v in args.drain(..) {
            self.windows.write(at, v);
            at += 1;
        }
        if let Some((spec, values)) = &captures {
            for (j, dst) in spec.dst.iter().enumerate() {
                self.windows.write(*dst, values[j].clone());
            }
        }
        crate::argv::give(args);
        self.go_eval(body.code, module);
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
        module: usize,
    ) -> Result<(), Diagnostic> {
        if let Some(stmt) = stmts.get(next) {
            let code = stmt.code().clone();
            let span = code.span;
            self.push(
                Frame::BlockStep {
                    stmts,
                    next: next + 1,
                    tail,
                    module,
                },
                span,
            )?;
            self.go_eval(code, module);
            return Ok(());
        }
        match tail {
            Some(t) => self.go_eval(t, module),
            None => self.go_return(Value::Unit),
        }
        Ok(())
    }

    pub(crate) fn try_arms(
        &mut self,
        scrutinee: Value,
        arms: Rc<Vec<code::Arm>>,
        from: usize,
        module: usize,
        scrutinee_span: Span,
    ) -> Result<(), Diagnostic> {
        // A matching arm writes its bindings straight into its own slots; a rejected arm's
        // partial writes land in slots only that arm reads, so there is nothing to undo.
        let mut hit = None;
        for at in from..arms.len() {
            if self.match_pattern(&arms[at].pat, &scrutinee, module)? {
                hit = Some(at);
                break;
            }
        }
        match hit {
            None => Err(err_non_exhaustive(scrutinee_span, &scrutinee)),
            Some(at) => match &arms[at].guard {
                None => {
                    let body = arms[at].body.clone();
                    self.go_eval(body, module);
                    Ok(())
                }
                Some(guard) => {
                    let guard = guard.clone();
                    let guard_span = guard.span;
                    self.push(
                        Frame::MatchGuard {
                            scrutinee,
                            arms: arms.clone(),
                            at,
                            module,
                            scrutinee_span,
                        },
                        guard_span,
                    )?;
                    self.go_eval(guard, module);
                    Ok(())
                }
            },
        }
    }

    pub(crate) fn match_pattern(
        &mut self,
        pat: &Pat,
        value: &Value,
        module: usize,
    ) -> Result<bool, Diagnostic> {
        Ok(match pat {
            Pat::Wildcard => true,
            Pat::Var { name, slot } => {
                // A nullary constructor written bare is indistinguishable from a binder in the
                // AST, so the constructor table decides — and a constructor binds nothing, which
                // is why its slot stays vacant and a read of the name falls back to global
                // resolution.
                let declared = self.ctor_name(module, &QName::bare(name.clone()));
                match declared.as_ref().and_then(|n| self.ctors.get(n)) {
                    Some(0) => {
                        let ctor = declared.expect("a hit came from a resolved name");
                        matches!(value, Value::Ctor { name, args }
                            if *name == ctor && args.is_empty())
                    }
                    _ => {
                        if let Some(s) = slot {
                            self.windows.write(*s, value.clone());
                        }
                        true
                    }
                }
            }
            Pat::Lit(lit) => crate::semantics::lit_matches(lit, value),
            Pat::Ctor { name, args } => match value {
                Value::Ctor {
                    name: vname,
                    args: vargs,
                } => {
                    let expected = self.ctor_name(module, name);
                    if expected.as_ref() != Some(vname) || vargs.len() != args.len() {
                        return Ok(false);
                    }
                    for (p, v) in args.iter().zip(vargs.iter()) {
                        if !self.match_pattern(p, v, module)? {
                            return Ok(false);
                        }
                    }
                    true
                }
                _ => false,
            },
            Pat::Record { fields, rest } => match value {
                Value::Record(map) => {
                    if !*rest && map.len() != fields.len() {
                        return Ok(false);
                    }
                    for (name, p) in fields {
                        let Some(v) = map.get(&name.name).cloned() else {
                            return Ok(false);
                        };
                        if !self.match_pattern(p, &v, module)? {
                            return Ok(false);
                        }
                    }
                    true
                }
                _ => false,
            },
            Pat::List { items, rest } => match value {
                Value::List(xs) => {
                    let fits = match rest {
                        Some(_) => xs.len() >= items.len(),
                        None => xs.len() == items.len(),
                    };
                    if !fits {
                        return Ok(false);
                    }
                    for (p, v) in items.iter().zip(xs.iter()) {
                        if !self.match_pattern(p, v, module)? {
                            return Ok(false);
                        }
                    }
                    match rest {
                        Some(rest) => {
                            let tail = Value::list(xs[items.len()..].to_vec());
                            self.match_pattern(rest, &tail, module)?
                        }
                        None => true,
                    }
                }
                _ => false,
            },
        })
    }

    /// The module's own items and its selective imports, then the prelude — the resolution order
    /// the whole language is specified in. Locals never reach here: their occurrences resolve to
    /// slots at lowering.
    fn lookup(&mut self, q: &QName, module: usize) -> Result<Value, Diagnostic> {
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
                params,
                size: body.size,
                body: body.code,
                captures: code::no_captures(),
                captured: Rc::from(Vec::new()),
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
        stack.push_delimiter(delimiter.clone(), 0)
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

/// A binding the liveness analysis moved out of its slot, read afterwards.
#[cold]
#[inline(never)]
fn err_released(name: &dyn std::fmt::Display, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{name}` was read after reference counting dropped it"),
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

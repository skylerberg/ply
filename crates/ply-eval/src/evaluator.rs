//! The evaluator (ADR 0047): one engine with two front ends over the shared runtime. The
//! interpreted front end ([`crate::interp::Core`]) walks the lowered `code` directly — the
//! first-order language, `with_cell`, tail-resumptive `handle`/`perform` — with no C compiler in
//! the path. Anything it declines — a `simulate`, a region's tasks, a multi-shot `resume`, a host
//! operation — runs on the compiled front end (the C tier attached with [`Machine::set_compiled`]),
//! which carries the ADR 0044 stacks that hold those continuations. Between them they answer every
//! body; there is no control-stack machine of a third kind.
//!
//! The engine is still called `Machine` so its consumers — the CLI, the harness, the prover, the
//! corpus — are unchanged. It records each entry point's performed atoms into a [`Trace`] so its
//! footprint reads the same whichever front end ran the body.

use crate::arena::RegionKind;
use crate::compiled::{Compiled, Entered};
use crate::host::{HostBinding, HostRuntime, HostUse, MachineId, Pending};
use crate::interp::{Core, Interpreter, Run};
use crate::limit::DEFAULT_MAX_CALLS;
use crate::region;
use crate::sim::{DEFAULT_STEPS, Seed};
use crate::trace::Trace;
use crate::value::Value;
use crate::{Arena, TaskRegions, code};
use ply_core::CheckOutput;
use ply_core::ty::{EffectAtom, Footprint};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{Expr, Item, Program};
use ply_syntax::resolve::Resolved;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

/// Ordered as [`CheckOutput::tests`] is — load order, then source order — so the index into the
/// two is the same index.
struct TestSlot<'a> {
    module: usize,
    name: &'a str,
    body: &'a Expr,
}

pub struct Machine<'a> {
    /// This engine's identity, which with the performing task is what a host handler keys scoped
    /// state on.
    id: MachineId,
    program: &'a Program,
    resolved: &'a Resolved,
    check: Option<&'a CheckOutput>,
    /// The interpreted front end's program tables and its eval state.
    interp: Interpreter<'a>,
    core: Core<'a>,
    tests: Vec<TestSlot<'a>>,
    /// Which of the region-kind rule's two kinds each region in this program is.
    region_kinds: crate::region_kind::Kinds,
    /// What this entry point performed, recorded whichever front end ran it.
    trace: Trace,
    max_calls: usize,
    /// The seed the next entry point's `simulate` region runs at, and the scheduling-step budget
    /// one interleaving may spend.
    seed: Seed,
    sim_steps: u32,
    /// The handler of last resort.
    binding: Arc<HostBinding>,
    /// What answers a [`crate::host::HostAnswer::Pending`].
    runtime: Option<Rc<dyn HostRuntime>>,
    /// The compiled front end, for the bodies the interpreter declines.
    compiled: Option<Rc<dyn Compiled>>,
    /// The compiled tier is the only engine: a body the interpreter declines and the tier does not
    /// hold is a failure.
    tier_only: bool,
    compiled_entries: Cell<u64>,
    compiled_declines: Cell<u64>,
    compiled_refusals: Cell<u64>,
    /// What this entry point's `simulate` regions did, read from the compiled front end.
    record: Option<region::Record>,
    /// What this entry point reached across the host boundary.
    host_use: HostUse,
    host_ops: u64,
    /// The declared footprint of the entry point about to run.
    declared: Option<Footprint>,
    re_executed: bool,
    /// What the runtime reported while closing entry points.
    teardown: Vec<Diagnostic>,
}

impl<'a> Machine<'a> {
    pub fn new(
        program: &'a Program,
        resolved: &'a Resolved,
        check: &'a CheckOutput,
    ) -> Machine<'a> {
        Machine::build(program, resolved, Some(check))
    }

    /// Everything the engine needs is derivable from the resolved AST alone, so evaluation can be
    /// exercised without a type-check pass.
    pub fn for_program(program: &'a Program, resolved: &'a Resolved) -> Machine<'a> {
        Machine::build(program, resolved, None)
    }

    fn build(
        program: &'a Program,
        resolved: &'a Resolved,
        check: Option<&'a CheckOutput>,
    ) -> Machine<'a> {
        let mut tests = Vec::new();
        for (m, module) in program.modules.iter().enumerate() {
            for item in &module.items {
                if let Item::Test(t) = item {
                    tests.push(TestSlot {
                        module: m,
                        name: t.name.as_str(),
                        body: &t.body,
                    });
                }
            }
        }
        Machine {
            id: MachineId::next(),
            program,
            resolved,
            check,
            interp: Interpreter::borrow(program, resolved),
            core: Core::new(program),
            tests,
            region_kinds: crate::region_kind::Kinds::default(),
            trace: Trace::new(),
            max_calls: DEFAULT_MAX_CALLS,
            seed: Seed::default(),
            sim_steps: DEFAULT_STEPS,
            binding: Arc::new(HostBinding::hermetic()),
            runtime: None,
            compiled: None,
            tier_only: false,
            compiled_entries: Cell::new(0),
            compiled_declines: Cell::new(0),
            compiled_refusals: Cell::new(0),
            record: None,
            host_use: HostUse::default(),
            host_ops: 0,
            declared: None,
            re_executed: false,
            teardown: Vec::new(),
        }
    }

    pub fn with_max_calls(mut self, max: usize) -> Machine<'a> {
        self.max_calls = max.max(1);
        self
    }

    pub fn set_max_calls(&mut self, max: usize) {
        self.max_calls = max.max(1);
    }

    /// Bind the host boundary.
    pub fn set_host_binding(&mut self, binding: Arc<HostBinding>) {
        self.binding = binding;
        self.share_host();
    }

    /// The compiled front end performs against the same binding, reactor and declared footprint
    /// this engine does, whichever was set last.
    fn share_host(&self) {
        if let Some(backend) = &self.compiled {
            backend.set_host(Arc::clone(&self.binding), self.runtime.clone());
            backend.set_declared(self.declared.clone());
            backend.set_re_executed(self.re_executed);
        }
    }

    /// The reactor a [`crate::host::HostAnswer::Pending`] is polled on.
    pub fn set_host_runtime(&mut self, runtime: Rc<dyn HostRuntime>) {
        self.runtime = Some(runtime);
        self.share_host();
    }

    pub fn host_binding(&self) -> &HostBinding {
        &self.binding
    }

    pub fn host_ops(&self) -> u64 {
        self.host_ops
    }

    pub fn set_declared_footprint(&mut self, footprint: Footprint) {
        self.declared = Some(footprint);
        self.share_host();
    }

    /// Declare that this entry point is one of several runs of the same test, so that reaching the
    /// host boundary is [`codes::HOST_IN_SIMULATION`] rather than a packet sent once per
    /// interleaving.
    pub fn set_re_executed(&mut self, re_executed: bool) {
        self.re_executed = re_executed;
        self.share_host();
    }

    pub fn host_use(&self) -> Option<&HostUse> {
        (!self.host_use.is_empty()).then_some(&self.host_use)
    }

    pub fn set_seed(&mut self, seed: Seed, steps: u32) {
        self.seed = seed;
        self.sim_steps = steps.max(1);
    }

    pub fn simulated(&self) -> Option<&region::Record> {
        self.record.as_ref()
    }

    pub fn program(&self) -> &'a Program {
        self.program
    }

    pub fn check(&self) -> Option<&'a CheckOutput> {
        self.check
    }

    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    pub fn cells(&self) -> &Arena {
        self.core.cells()
    }

    pub fn cells_mut(&mut self) -> &mut Arena {
        self.core.cells_mut()
    }

    pub fn regions(&self) -> &TaskRegions {
        self.core.regions()
    }

    /// The kind of the region opened at `span`, and `None` when that span opens no region.
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

    pub fn share_region_kinds(&mut self, kinds: crate::region_kind::Kinds) {
        self.region_kinds = kinds;
    }

    /// The lowering cache to hand an engine built next over **this same program**.
    pub fn share_lowering(&self) -> Rc<crate::code::Lowering<'a>> {
        self.core.lowering()
    }

    pub fn set_lowering(&mut self, lowering: Rc<crate::code::Lowering<'a>>) {
        if lowering.describes(self.program) {
            self.core.set_lowering(lowering);
        }
    }

    /// Attach the compiled front end: the source of the bodies the interpreter declines.
    pub fn set_compiled(&mut self, compiled: Rc<dyn Compiled>) {
        if compiled.describes(self.program) {
            self.tier_only = compiled.tier_only();
            self.compiled = Some(compiled);
            self.share_host();
        }
    }

    pub fn set_tier_only(&mut self, tier_only: bool) {
        self.tier_only = tier_only;
    }

    pub fn compiled_counts(&self) -> (u64, u64) {
        (self.compiled_entries.get(), self.compiled_declines.get())
    }

    pub fn compiled_refusals(&self) -> u64 {
        self.compiled_refusals.get()
    }

    /// Every subsequent entry point resets to this stack's fixture rather than to an empty one.
    pub fn set_regions(&mut self, regions: TaskRegions) {
        self.core.set_regions(regions);
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
        let module = slot.module;
        let ordinal = self.tests[..index]
            .iter()
            .filter(|t| t.module == module)
            .count();
        let name = self.program.modules[module].name.as_symbol().clone();
        self.eval_test_in(&name, ordinal)
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
            .map(|slot| (slot.module, slot.body, slot.name));
        let Some((owner, body, label)) = found else {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("module `{module}` has no test at position {ordinal}"),
            )
            .primary(Span::DUMMY, "this test's module was not parsed")
            .note("run `ply cache clear`, or pass `--no-incremental`"));
        };
        self.begin_entry();
        // Tier-only: the compiled tier runs the language, so a test is entered on it directly.
        self.tier_test(owner, ordinal, label, body.span)
    }

    /// A test the interpreter declined, run on the compiled front end, which is the authority: its
    /// unit answer is the pass, its raise the failure, and a body it does not hold is a failure the
    /// interpreter could not answer either.
    fn tier_test(
        &mut self,
        owner: usize,
        ordinal: usize,
        label: &str,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let root = self.program.modules[owner]
            .name
            .qualify(&Symbol::new(format!("test#{ordinal}")));
        let Some(backend) = self.compiled.clone() else {
            return Err(err_no_front_end(&root, span));
        };
        backend.set_seed(self.seed.clone(), self.sim_steps);
        let entered = backend.enter_test(&root, self.max_calls);
        self.record_compiled_atoms();
        self.record = backend.simulated();
        let out = match entered {
            Entered::Answered(Value::Unit) => {
                self.compiled_entries.set(self.compiled_entries.get() + 1);
                Ok(())
            }
            Entered::Answered(_) => {
                self.compiled_refusals.set(self.compiled_refusals.get() + 1);
                self.compiled_declines.set(self.compiled_declines.get() + 1);
                Err(err_no_front_end(&root, span))
            }
            Entered::Raised(raised) => {
                self.compiled_declines.set(self.compiled_declines.get() + 1);
                Err(raised)
            }
            Entered::Declined => {
                self.compiled_declines.set(self.compiled_declines.get() + 1);
                Err(err_no_front_end(&root, span))
            }
        };
        let _ = label;
        self.end_entry_point();
        out
    }

    /// An expression of unknown provenance, lowered afresh and run on the interpreter.
    pub fn eval_expr_for_test(&mut self, e: &Expr) -> Result<Value, Diagnostic> {
        self.begin_entry();
        let lowered = code::lower(e);
        let entered =
            Run::new(&self.interp, &mut self.core).enter_lowered(lowered, 0, &[], self.max_calls);
        self.answer_expr(entered)
    }

    /// An expression from `module`, with `bindings` already in scope: the names are lowered as
    /// leading parameters of the body's window, so their occurrences resolve to slots exactly as a
    /// function's parameters do.
    pub fn eval_expr_in(
        &mut self,
        e: &'a Expr,
        module: usize,
        bindings: &[(Symbol, Value)],
    ) -> Result<Value, Diagnostic> {
        self.begin_entry();
        let entered = Run::new(&self.interp, &mut self.core).enter_expr_in(
            e,
            bindings,
            module,
            self.max_calls,
        );
        self.answer_expr(entered)
    }

    fn answer_expr(&mut self, entered: Entered) -> Result<Value, Diagnostic> {
        match entered {
            Entered::Answered(v) => {
                self.record_core_atoms();
                self.end_entry_point();
                Ok(v)
            }
            Entered::Raised(d) => {
                self.record_core_atoms();
                self.end_entry_point();
                Err(d)
            }
            Entered::Declined => {
                self.end_entry_point();
                Err(Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    "this expression uses a construct only the compiled tier evaluates",
                )
                .note("an arbitrary expression has no compiled body to fall back to"))
            }
        }
    }

    /// `name` is the program-wide name — `app.main`, not `main`.
    pub fn call(&mut self, name: &str, args: Vec<Value>, span: Span) -> Result<Value, Diagnostic> {
        let sym = Symbol::new(name);
        // Before the run resets, so a refusal leaves the previous run's arena alone.
        let boundary = crate::escape::Boundary::EntryPoint { name };
        for arg in &args {
            crate::escape::check(&boundary, arg, span)?;
        }
        self.begin_entry();
        // Tier-only: an entry point is run on the compiled tier.
        self.tier_call(&sym, args, span)
    }

    /// The same call a nested engine is handed mid-run; the entry-point escape check is the same
    /// one `call` runs, and a nested engine's arguments have already been admitted.
    pub(crate) fn call_within(
        &mut self,
        name: &str,
        args: Vec<Value>,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        self.call(name, args, span)
    }

    fn tier_call(
        &mut self,
        sym: &Symbol,
        args: Vec<Value>,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let Some(backend) = self.compiled.clone() else {
            return Err(err_no_front_end(sym, span));
        };
        backend.set_seed(self.seed.clone(), self.sim_steps);
        let entered = backend.enter_whole(sym, &args, self.max_calls);
        self.record_compiled_atoms();
        self.record = backend.simulated();
        self.end_entry_point();
        match entered {
            Entered::Answered(value) => {
                self.compiled_entries.set(self.compiled_entries.get() + 1);
                Ok(value)
            }
            Entered::Raised(raised) => Err(raised),
            Entered::Declined => Err(err_no_front_end(sym, span)),
        }
    }

    /// Clear the per-entry accounting the next run overwrites; the interpreter's `Core` resets its
    /// own arena and handler stack when it enters.
    fn begin_entry(&mut self) {
        self.trace.clear();
        self.host_use = HostUse::default();
        self.host_ops = 0;
        self.record = None;
    }

    fn record_core_atoms(&mut self) {
        for atom in self.core.take_performed() {
            self.trace.record(atom);
        }
    }

    fn record_compiled_atoms(&mut self) {
        let Some(backend) = self.compiled.as_ref() else {
            return;
        };
        for atom in backend.take_performed() {
            self.trace.record(atom);
        }
        let (used, ops) = backend.take_host_use();
        self.host_use.atoms = self.host_use.atoms.union(&used.atoms);
        self.host_use.operations += used.operations;
        self.host_ops = self.host_ops.saturating_add(ops);
        self.teardown.extend(backend.take_teardown());
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
}

fn err_no_front_end(name: &Symbol, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("neither front end holds a body for `{name}`"),
    )
    .primary(
        span,
        "the interpreter declined it and the compiled tier has no body for it",
    )
    .note(
        "attach the compiled tier, or the construct this body uses is one no front end carries yet",
    )
}

/// A `simulate` region entered inside another one.
pub fn err_nested_simulation(span: Span, outer: Span) -> Diagnostic {
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

/// A `HostAnswer::Pending` with no reactor to resolve it.
#[cold]
#[inline(never)]
pub fn err_no_runtime(
    span: Span,
    operation: &str,
    pending: Pending,
    path: &'static str,
) -> Diagnostic {
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
pub fn err_unenumerated_atom(span: Span, operation: &str, path: &'static str) -> Diagnostic {
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
pub fn err_footprint_escape(
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
pub fn check_host_answer(
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

pub fn carries_secret(v: &Value) -> bool {
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
pub fn err_secret_to_host(
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
pub fn err_host_in_simulation(span: Span, operation: &str, region: Span) -> Diagnostic {
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

/// A runtime for a context that has none: every wait is a failure that names itself.
pub struct Unbound;

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
    .primary(Span::DUMMY, "no reactor is bound to this engine")
    .note("`Machine::set_host_runtime` was never called, so only handlers that answer a value outright can run here")
}

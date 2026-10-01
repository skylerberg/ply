//! The engine: entry points and tests run on the compiled tier; performed atoms go to one
//! [`Trace`].

use crate::compiled::{Compiled, Entered};
use crate::host::{HostBinding, HostRuntime, HostUse, MachineId, Pending};
use crate::limit::DEFAULT_MAX_CALLS;
use crate::sim::{DEFAULT_STEPS, Seed};
use crate::trace::Trace;
use crate::value::Value;
use crate::{
    Arena, DefHash, Diagnostic, EffectAtom, Footprint, Front, ModuleName, Span, Symbol,
    TaskRegions, codes, region,
};
use std::rc::Rc;
use std::sync::Arc;

pub struct Machine<'a> {
    /// With the performing task, the key a host handler scopes its state by.
    id: MachineId,
    /// Its tests are [`Machine::eval_test`]'s indices; its hashes name the unit it may enter.
    front: &'a Front,
    regions: TaskRegions,
    trace: Trace,
    max_calls: usize,
    /// Seed and per-interleaving step budget for the next entry point's `simulate` regions.
    seed: Seed,
    sim_steps: u32,
    /// The handler of last resort.
    binding: Arc<HostBinding>,
    /// What answers a [`crate::host::HostAnswer::Pending`], made on this machine's thread, and what
    /// made it, for the threads a `parallel` block runs branches on.
    runtime: Option<Rc<dyn HostRuntime>>,
    factory: Option<crate::host::RuntimeFactory>,
    /// The only evaluator: every entry point and test runs here.
    compiled: Rc<dyn Compiled>,
    compiled_entries: u64,
    compiled_declines: u64,
    record: Option<region::Record>,
    host_use: HostUse,
    host_ops: u64,
    declared: Option<Footprint>,
    re_executed: bool,
}

/// How one entry ended: its answer, and what the host runtime warned of as it ended, such as the
/// spans it left open. The answer is not had without the warnings in hand.
#[must_use = "an entry's warnings are its driver's to report"]
#[derive(Debug)]
pub struct Ended<T> {
    answer: Result<T, Diagnostic>,
    warnings: Vec<Diagnostic>,
}

impl<T> Ended<T> {
    /// Refused before anything was entered, so nothing ended and nothing warns.
    pub fn refused(diagnostic: Diagnostic) -> Ended<T> {
        Ended {
            answer: Err(diagnostic),
            warnings: Vec::new(),
        }
    }

    /// The answer made into another, the warnings kept with it.
    pub fn map<U>(
        self,
        f: impl FnOnce(Result<T, Diagnostic>) -> Result<U, Diagnostic>,
    ) -> Ended<U> {
        Ended {
            answer: f(self.answer),
            warnings: self.warnings,
        }
    }

    pub fn into_parts(self) -> (Result<T, Diagnostic>, Vec<Diagnostic>) {
        (self.answer, self.warnings)
    }
}

impl<'a> Machine<'a> {
    /// Refuses a tier built from another program, whose bodies would answer for that one.
    pub fn new(front: &'a Front, compiled: Rc<dyn Compiled>) -> Result<Machine<'a>, Diagnostic> {
        if !compiled.describes(front.hashes_digest) {
            return Err(err_foreign_tier(front.hashes_digest));
        }
        let machine = Machine {
            id: MachineId::next(),
            front,
            regions: TaskRegions::new(),
            trace: Trace::new(),
            max_calls: DEFAULT_MAX_CALLS,
            seed: Seed::default(),
            sim_steps: DEFAULT_STEPS,
            binding: Arc::new(HostBinding::hermetic()),
            runtime: None,
            factory: None,
            compiled,
            compiled_entries: 0,
            compiled_declines: 0,
            record: None,
            host_use: HostUse::default(),
            host_ops: 0,
            declared: None,
            re_executed: false,
        };
        // A tier may be shared by several machines, so it takes this one's hermetic defaults.
        machine.share_host();
        Ok(machine)
    }

    pub fn with_max_calls(mut self, max: usize) -> Machine<'a> {
        self.max_calls = max.max(1);
        self
    }

    pub fn set_host_binding(&mut self, binding: Arc<HostBinding>) {
        self.binding = binding;
        self.share_host();
    }

    fn share_host(&self) {
        self.compiled.set_host(
            Arc::clone(&self.binding),
            self.runtime.clone(),
            self.factory.clone(),
        );
        self.compiled.set_declared(self.declared.clone());
        self.compiled.set_re_executed(self.re_executed);
    }

    pub fn set_host_runtime(&mut self, factory: crate::host::RuntimeFactory) {
        self.runtime = Some(factory());
        self.factory = Some(factory);
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

    /// One of several runs of one test: reaching the host is [`codes::HOST_IN_SIMULATION`].
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

    pub fn trace(&self) -> &Trace {
        &self.trace
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

    /// Entries the tier ran, and entries it declined, over this machine's life.
    pub fn compiled_counts(&self) -> (u64, u64) {
        (self.compiled_entries, self.compiled_declines)
    }

    /// Every subsequent entry point resets to this stack's fixture rather than to an empty one.
    pub fn set_regions(&mut self, regions: TaskRegions) {
        self.regions = regions;
    }

    pub fn test_count(&self) -> usize {
        self.front.check.tests.len()
    }

    pub fn test_name(&self, index: usize) -> Option<&'a str> {
        self.front.check.tests.get(index).map(|t| t.name.as_str())
    }

    /// `index` into the front's tests: load order, then source order.
    pub fn eval_test(&mut self, index: usize) -> Ended<()> {
        let front = self.front;
        let tests = &front.check.tests;
        let Some(test) = tests.get(index) else {
            return Ended::refused(
                Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    format!(
                        "no test at index {index}; the program defines {}",
                        tests.len()
                    ),
                )
                .primary(Span::DUMMY, "requested test does not exist"),
            );
        };
        let ordinal = tests[..index]
            .iter()
            .filter(|t| t.module == test.module)
            .count();
        self.begin_entry();
        self.tier_test(&test.module, ordinal, test.span)
    }

    /// The compiled front end is the authority: unit passes, a raise fails, and a missing body or
    /// any other answer is Ply's defect.
    fn tier_test(&mut self, module: &ModuleName, ordinal: usize, span: Span) -> Ended<()> {
        let root = module.qualify(&Symbol::new(format!("test#{ordinal}")));
        self.compiled.set_seed(self.seed.clone(), self.sim_steps);
        let entered = self.compiled.enter_test(&root, self.max_calls);
        self.record_compiled_atoms();
        self.record = self.compiled.simulated();
        let answer = match entered {
            Entered::Answered(Value::Unit) => {
                self.compiled_entries += 1;
                Ok(())
            }
            Entered::Answered(value) => {
                self.compiled_entries += 1;
                Err(err_test_answered(&root, &value, span))
            }
            Entered::Declined => {
                self.compiled_declines += 1;
                Err(err_not_compiled(&root, span))
            }
            Entered::Raised(raised) => {
                self.compiled_entries += 1;
                Err(raised)
            }
        };
        Ended {
            answer,
            warnings: self.end_entry_point(),
        }
    }

    /// `name` is the program-wide name — `app.main`, not `main`.
    pub fn call(&mut self, name: &str, args: Vec<Value>, span: Span) -> Ended<Value> {
        let sym = Symbol::new(name);
        // Before the run resets, so a refusal leaves the previous run's arena alone.
        let boundary = crate::escape::Boundary::EntryPoint { name };
        for arg in &args {
            if let Err(refused) = crate::escape::check(&boundary, arg, span) {
                return Ended::refused(refused);
            }
        }
        self.begin_entry();
        self.tier_call(&sym, args, span)
    }

    fn tier_call(&mut self, sym: &Symbol, args: Vec<Value>, span: Span) -> Ended<Value> {
        self.compiled.set_seed(self.seed.clone(), self.sim_steps);
        let entered = self.compiled.enter_whole(sym, &args, self.max_calls);
        self.record_compiled_atoms();
        self.record = self.compiled.simulated();
        let warnings = self.end_entry_point();
        let answer = match entered {
            Entered::Answered(value) => {
                self.compiled_entries += 1;
                Ok(value)
            }
            Entered::Raised(raised) => {
                self.compiled_entries += 1;
                Err(raised)
            }
            Entered::Declined => {
                self.compiled_declines += 1;
                Err(err_not_compiled(sym, span))
            }
        };
        Ended { answer, warnings }
    }

    fn begin_entry(&mut self) {
        self.trace.clear();
        self.host_use = HostUse::default();
        self.host_ops = 0;
        self.record = None;
    }

    fn record_compiled_atoms(&mut self) {
        for atom in self.compiled.take_performed() {
            self.trace.record(atom);
        }
        let (used, ops) = self.compiled.take_host_use();
        self.host_use.atoms = self.host_use.atoms.union(&used.atoms);
        self.host_use.operations += used.operations;
        self.host_ops = self.host_ops.saturating_add(ops);
    }

    /// Hands the host runtime every exit path from an entry point, and answers what the entry's
    /// ending warned of.
    fn end_entry_point(&mut self) -> Vec<Diagnostic> {
        let mut warnings = self.compiled.take_teardown();
        if let Some(runtime) = self.runtime.clone() {
            warnings.extend(runtime.end_entry_point(self.id));
        }
        warnings
    }
}

pub fn err_not_compiled(name: &Symbol, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the compiled tier declined to enter `{name}`"),
    )
    .primary(span, "the compiled tier declined this")
    .note(
        "a body the emitter cannot compile is `E0448` where the program is built, so this is the \
         seam rather than the body: a unit that failed to build, a signature the boundary does \
         not carry, the wrong number of arguments, or an entry reached while another was running",
    )
    .note("this is Ply's fault, not the program's")
}

#[cold]
#[inline(never)]
fn err_test_answered(root: &Symbol, value: &Value, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "test root `{root}` answered `{}`, and a test body is `Unit`",
            crate::slot(0)
        ),
    )
    .primary(
        span,
        "the compiled tier ran this test and it answered a value",
    )
    .note(
        "the checker refuses a test whose body is not `Unit` with E0201, so no test can answer one",
    )
    .note("this is Ply's fault, not the program's")
    .showing(vec![crate::Plain::shown(value)])
}

#[cold]
#[inline(never)]
fn err_foreign_tier(program: DefHash) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a machine for program {program} was handed a compiled tier built from another"),
    )
    .primary(Span::DUMMY, "nothing was run")
    .note("a unit's bodies answer for the program it was compiled from, never for another")
    .note("this is Ply's fault, not the program's")
}

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

/// `E0427`: the binding enumerated no atom for an operation a registration claims.
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
    fn watch(&self, pending: &Pending) -> Result<(), Diagnostic> {
        Err(err_unbound_runtime(&format!("watch `{pending}`")))
    }

    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        Vec::new()
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

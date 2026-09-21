//! The machine entering natively compiled code, from a command a user runs.

use crate::rt::Entry;
use crate::source::Source;
use anyhow::{Context, Result, bail};
use ply_eval::{Compilation, Counters, Entered, Provider, Value};
use ply_span::{Diagnostic, Symbol};
use ply_ty::DefHash;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// The widest arity this boundary carries without allocating an argument array.
const MAX_ARITY: usize = 16;

/// One admitted definition: where its code is, and how many arguments it takes.
struct Admitted {
    entry: Entry,
    arity: usize,
    /// The memo index of a pure nullary root.
    constant: Option<usize>,
}

/// Why an offered call was not taken.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Declines {
    /// The machine offered a name this unit did not compile.
    pub not_compiled: u64,
    /// It compiled the name and the call had the wrong number of arguments.
    pub arity: u64,
    /// An entry arrived while another was running.
    pub reentered: u64,
    /// A builtin touched cells, so the compile-time refusal of `cell_get`/`cell_set` has a hole.
    pub touched_cells: u64,
    /// The answer held a closure, cell, task, continuation or secret, which cannot cross out.
    pub answer: u64,
}

impl Declines {
    pub fn total(&self) -> u64 {
        self.not_compiled + self.arity + self.reentered + self.touched_cells + self.answer
    }
}

/// One run's compiled unit, shared by every worker's backend.
pub struct Unit {
    /// [`ply_ty::HashOutput::digest`] of the program this was built over, for `Compiled::describes`.
    identity: DefHash,
    source: &'static Source,
    /// The set the emitter compiles as one unit, closed under calls.
    compiled: Vec<String>,
    /// The subset of `compiled` the machine may be offered.
    members: BTreeSet<Symbol>,
    /// Definitions the emitter refused, with the construct that refused each.
    refusals: Vec<(String, String)>,
    counters: Counters,
    /// Nanoseconds the pre-flight build took, paid once; workers read the unit back.
    analysis_nanos: u64,
    codegen_nanos: AtomicU64,
    compiles: AtomicU64,
    /// Workers whose build failed after the pre-flight in [`Unit::over_front`] succeeded.
    poisoned: AtomicU64,
    /// An artifact's self-describing C, loaded rather than built.
    embedded: Option<String>,
}

impl Unit {
    /// `texts` is each module's source by name, which the cache keys cover.
    pub fn over_front(
        front: &ply_ty::Front,
        texts: HashMap<String, String>,
    ) -> Result<&'static Unit> {
        let identity = front.hashes.digest();
        let front: &'static ply_ty::Front = Box::leak(Box::new(front.clone()));
        let keys = crate::source::emit_keys(front);
        let source: &'static Source =
            Box::leak(Box::new(Source::from_front(front, keys).with_texts(texts)));
        let candidates = source.functions();
        let started = std::time::Instant::now();
        // The pre-flight decides the compiled set and leaves the unit every worker reads back.
        let (compiled, refusals) = closure(source, &candidates)?;
        let members: BTreeSet<Symbol> = compiled
            .iter()
            .filter(|name| registers(source, name))
            .map(Symbol::new)
            .collect();
        let analysis_nanos = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let unit = Unit {
            identity,
            source,
            compiled,
            members,
            refusals,
            counters: Counters::default(),
            analysis_nanos,
            codegen_nanos: AtomicU64::new(0),
            compiles: AtomicU64::new(0),
            poisoned: AtomicU64::new(0),
            embedded: None,
        };
        Ok(Box::leak(Box::new(unit)))
    }

    /// An artifact's unit, produced elsewhere; loaded once here to read its table.
    pub fn embedded(front: &ply_ty::Front, text: String) -> Result<&'static Unit> {
        let exports = crate::c::Exports::read(&crate::c::compile_and_load(&text, "artifact")?)?;
        let identity = front.hashes.digest();
        let front: &'static ply_ty::Front = Box::leak(Box::new(front.clone()));
        let source: &'static Source =
            Box::leak(Box::new(Source::from_front(front, HashMap::new())));
        let compiled = exports.names();
        let members: BTreeSet<Symbol> = compiled
            .iter()
            .filter(|name| registers(source, name))
            .map(Symbol::new)
            .collect();
        let unit = Unit {
            identity,
            source,
            compiled,
            members,
            refusals: exports.refusals,
            counters: Counters::default(),
            analysis_nanos: 0,
            codegen_nanos: AtomicU64::new(0),
            compiles: AtomicU64::new(0),
            poisoned: AtomicU64::new(0),
            embedded: Some(text),
        };
        Ok(Box::leak(Box::new(unit)))
    }

    /// The whole unit over `names`, produced and not compiled: what `ply build` embeds.
    pub fn produce(&'static self, names: &[&str]) -> Result<crate::c::Produced> {
        crate::c::produce(self.source, names)
    }

    /// The constructor table a unit over this program is emitted against.
    pub fn ctors(&self) -> Vec<(Symbol, usize)> {
        self.source.ctors()
    }

    /// The bodies this unit builds, as the concrete type rather than `dyn Compiled`, for tests.
    pub fn bodies(&'static self) -> Result<Rc<Bodies>> {
        self.build().map(Rc::new)
    }

    /// The definitions the emitter compiles, closed under calls.
    pub fn compiled(&self) -> &[String] {
        &self.compiled
    }

    /// What the emitter refused and the construct that refused it.
    pub fn refusals(&self) -> &[(String, String)] {
        &self.refusals
    }

    /// What this unit has spent compiling, in its two halves.
    pub fn compilation(&self) -> Compilation {
        Compilation {
            analysis_nanos: self.analysis_nanos,
            codegen_nanos: self.codegen_nanos.load(Ordering::Relaxed),
            units: self.compiles.load(Ordering::Relaxed),
        }
    }

    pub fn poisoned(&self) -> u64 {
        self.poisoned.load(Ordering::Relaxed)
    }

    fn build(&'static self) -> Result<Bodies> {
        let started = std::time::Instant::now();
        let native = match &self.embedded {
            Some(text) => crate::c::load_unit(text, Some(self.source), "artifact")?.0,
            // The same set as the pre-flight, so the unit key matches and the unit is read back.
            None => {
                let candidates = self.source.functions();
                let names: Vec<&str> = candidates.iter().map(String::as_str).collect();
                crate::c::build(self.source, &names)?.0
            }
        };
        self.codegen_nanos.fetch_add(
            u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.compiles.fetch_add(1, Ordering::Relaxed);
        Bodies::new(self, native)
    }
}

impl Provider for Unit {
    fn attach(&'static self, _spec: &ply_eval::BackendSpec) -> Rc<dyn ply_eval::Compiled> {
        if self.members.is_empty() {
            return Rc::new(Absent { unit: self });
        }
        match self.build() {
            Ok(bodies) => Rc::new(bodies),
            Err(e) => {
                eprintln!("the C tier built no unit for this program: {e:#}");
                self.poisoned.fetch_add(1, Ordering::Relaxed);
                Rc::new(Absent { unit: self })
            }
        }
    }

    fn name(&self) -> &'static str {
        "c"
    }

    fn len(&self) -> usize {
        self.members.len()
    }

    fn offers(&self) -> ply_eval::Offers {
        self.counters.offers()
    }

    fn compilation(&self) -> Option<Compilation> {
        Some(Unit::compilation(self))
    }

    fn unbuilt(&self) -> u64 {
        self.poisoned()
    }

    fn relocate(&self, front: &ply_ty::Front, sources: &ply_span::SourceMap) -> bool {
        self.source.relocate(front, sources)
    }
}

/// A worker whose compile failed: it declines everything and is counted.
struct Absent {
    unit: &'static Unit,
}

impl ply_eval::Compiled for Absent {
    fn describes(&self, program: DefHash) -> bool {
        self.unit.identity == program
    }

    fn enter(&self, _name: &Symbol, args: &[Value], _budget: usize) -> Option<Value> {
        self.unit.counters.note_offer(args);
        None
    }
}

/// One worker's compiled bodies, offered to a `Machine` through `ply_eval::Compiled`.
pub struct Bodies {
    unit: &'static Unit,
    /// Kept alive because every [`Entry`] below points into its executable pages.
    _code: crate::c::Native,
    admitted: HashMap<Symbol, Admitted>,
    /// One context for every entry; the `RefCell` forbids nesting, since `Ctx::slots` has no pop.
    ctx: RefCell<crate::rt::Ctx>,
    entered: Cell<u64>,
    declines: Cell<Declines>,
    /// `PLY_TIER_ONLY=1`: this backend is the only engine, and the machine evaluates nothing.
    tier_only: bool,
}

impl Bodies {
    fn new(unit: &'static Unit, code: crate::c::Native) -> Result<Bodies> {
        if let Some(what) = code.tables().retains_a_handle() {
            bail!(
                "the constant pool holds {what}, which must not outlive the call that made it; \
                 refusing the whole registration rather than entering anything"
            );
        }
        let mut admitted = HashMap::new();
        for name in &unit.members {
            let Some(entry) = code.entry(name.as_str()) else {
                bail!("`{name}` was admitted and not compiled");
            };
            let Some(arity) = code.arity(name.as_str()) else {
                bail!("`{name}` was compiled without an arity");
            };
            if arity > MAX_ARITY {
                bail!(
                    "`{name}` takes {arity} arguments and this boundary carries {MAX_ARITY}; \
                     refusing the registration rather than leaving one name that declines \
                     every call and no reason recorded against it"
                );
            }
            // Fixed widths are held as tagged `Int`s, so one crossing the seam would read wrongly.
            if unit
                .source
                .check
                .defs
                .get(name)
                .is_some_and(|def| ply_eval::mentions_a_width(&def.scheme.ty))
            {
                continue;
            }
            let constant = code.constant_index(name.as_str());
            admitted.insert(
                name.clone(),
                Admitted {
                    entry,
                    arity,
                    constant,
                },
            );
        }
        let ctx = RefCell::new(code.context());
        Ok(Bodies {
            unit,
            _code: code,
            admitted,
            ctx,
            entered: Cell::new(0),
            declines: Cell::new(Declines::default()),
            tier_only: std::env::var("PLY_TIER_ONLY").is_ok_and(|v| v == "1"),
        })
    }

    /// Native bodies actually run, over this backend's whole life.
    pub fn entered(&self) -> u64 {
        self.entered.get()
    }

    /// Runs `f` with the context borrowed, so a test can reach the reentrancy decline.
    pub fn while_entered<T>(&self, f: impl FnOnce() -> T) -> T {
        let _held = self.ctx.borrow_mut();
        f()
    }

    /// Whether this backend would be offered `name` at all.
    pub fn admits(&self, name: &str) -> bool {
        self.admitted.contains_key(&Symbol::new(name))
    }

    /// The cell arena's `(regions open, slots live)` between entries: what a leak grows.
    pub fn cell_extent(&self) -> (usize, usize) {
        self.ctx.borrow().cell_extent()
    }

    pub fn reset_counts(&self) {
        self.entered.set(0);
        self.declines.set(Declines::default());
    }

    pub fn declines(&self) -> Declines {
        self.declines.get()
    }

    fn decline(&self, mut f: impl FnMut(&mut Declines)) -> Run {
        let mut d = self.declines.get();
        f(&mut d);
        self.declines.set(d);
        Run::Declined
    }

    fn run(&self, name: &Symbol, args: &[Value], fuel: usize) -> Run {
        let Some(admitted) = self.admitted.get(name) else {
            return self.decline(|d| d.not_compiled += 1);
        };
        if admitted.arity != args.len() {
            return self.decline(|d| d.arity += 1);
        }
        let Ok(mut ctx) = self.ctx.try_borrow_mut() else {
            return self.decline(|d| d.reentered += 1);
        };

        let tables = Rc::clone(&ctx.tables);
        if let Some(index) = admitted.constant
            && let Some(kept) = tables.memoized(index)
            && let Some(value) = tables.memo_value(kept)
        {
            drop(ctx);
            self.unit.counters.note_converted(0, 0);
            self.entered.set(self.entered.get() + 1);
            return Run::Answered(value);
        }
        ctx.begin(i64::try_from(fuel).unwrap_or(i64::MAX));
        // Values are deep-converted in and out: nothing outside the entry ever holds a word.
        let mut handles = [0i64; MAX_ARITY];
        let before = ctx.heap.allocated();
        // A call whose arguments are all memoized words is itself memoized.
        let mut all_memo = !args.is_empty();
        for (slot, value) in handles.iter_mut().zip(args) {
            *slot = match tables.memo_word(value) {
                Some(w) => w,
                None => {
                    all_memo = false;
                    ctx.heap.to_word(&tables.layouts, value)
                }
            };
        }
        let words = &handles[..args.len()];
        if all_memo && let Some(value) = tables.memo_call(name, words) {
            ctx.end();
            drop(ctx);
            self.unit.counters.note_converted(0, 0);
            self.entered.set(self.entered.get() + 1);
            return Run::Answered(value);
        }
        let inward = (ctx.heap.allocated() - before) as u64;
        // SAFETY: `self._code` owns the entry's pages, `ctx` is uniquely borrowed, and
        // `handles` is `MAX_ARITY` wide, which `Bodies::new` refused to exceed.
        let mut out =
            unsafe { (admitted.entry)(&mut *ctx as *mut crate::rt::Ctx, handles.as_ptr()) };
        if ctx.sims.last().is_some_and(|sim| sim.is_production()) {
            out = unsafe { crate::simulate::finish_root(&mut *ctx as *mut crate::rt::Ctx, out) };
        }
        if let Some(rt) = ctx.runtime.clone()
            && let Err(d) = rt.end_entry_point(ctx.id)
        {
            ctx.teardown.push(d);
        }

        if ctx.failed != 0 {
            let out_of_stack = ctx.failed == crate::rt::FAILED_OUT_OF_STACK;
            let out_of_fuel = out_of_stack || ctx.failed == crate::rt::FAILED_OUT_OF_FUEL;
            let raised = if out_of_fuel {
                // Tier-only: no machine follows, so report the budget even on a stack overflow.
                Some(
                    ply_span::Diagnostic::error(
                        ply_span::codes::RUNTIME_ERROR,
                        format!("recursion limit of {fuel} nested calls exceeded"),
                    )
                    .primary(ctx.site(), "the call that overran it"),
                )
            } else {
                ctx.diagnostic.take().or_else(|| {
                    (ctx.failed == crate::rt::FAILED_UNWIND).then(|| {
                        ply_span::Diagnostic::error(
                            ply_span::codes::RUNTIME_ERROR,
                            "a `handle` clause unwound past the compiled fragment's entry",
                        )
                    })
                })
            };
            ctx.end();
            drop(ctx);
            self.entered.set(self.entered.get() + 1);
            return match raised {
                Some(raised) => Run::Raised(raised),
                None => Run::Declined,
            };
        }
        if !ctx.cells_balanced() {
            ctx.end();
            drop(ctx);
            return self.decline(|d| d.touched_cells += 1);
        }
        // Memoize the word and its converted value, so later entries skip both run and conversion.
        let mut walked = crate::heap::Walked::default();
        let value = crate::heap::Heap::to_value_counted(&tables.layouts, out, &mut walked);
        let kept = match admitted.constant {
            Some(index) if crate::heap::world_independent(out) => Some(tables.memoize(index, out)),
            None if all_memo && crate::heap::world_independent(out) => {
                tables.memoize_call(name, words, out)
            }
            _ => None,
        };
        ctx.end();
        drop(ctx);
        if walked.handle {
            return self.decline(|d| d.answer += 1);
        }
        if let Some(kept) = kept {
            tables.remember(kept, &value);
        }
        self.unit.counters.note_converted(inward, walked.read);
        self.entered.set(self.entered.get() + 1);
        Run::Answered(value)
    }
}

/// How one entry ended.
enum Run {
    Answered(Value),
    Raised(Diagnostic),
    Declined,
}

impl Run {
    fn answer(self) -> Option<Value> {
        match self {
            Run::Answered(value) => Some(value),
            Run::Raised(_) | Run::Declined => None,
        }
    }
}

impl ply_eval::Compiled for Bodies {
    fn describes(&self, program: DefHash) -> bool {
        self.unit.identity == program
    }

    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
        self.unit.counters.note_offer(args);
        self.run(name, args, budget).answer()
    }

    fn enter_test(&self, name: &Symbol, budget: usize) -> Entered {
        self.unit.counters.note_offer(&[]);
        match self.run(name, &[], budget) {
            Run::Answered(value) => Entered::Answered(value),
            Run::Raised(raised) => Entered::Raised(raised),
            Run::Declined => Entered::Declined,
        }
    }

    fn enter_whole(&self, name: &Symbol, args: &[Value], budget: usize) -> Entered {
        self.unit.counters.note_offer(args);
        match self.run(name, args, budget) {
            Run::Answered(value) => Entered::Answered(value),
            Run::Raised(raised) => Entered::Raised(raised),
            Run::Declined => Entered::Declined,
        }
    }

    // A borrowed context means a nested entry, which `run` declines, so there is nothing to take.
    fn take_performed(&self) -> Vec<ply_ty::EffectAtom> {
        self.ctx
            .try_borrow_mut()
            .map(|mut ctx| std::mem::take(&mut ctx.performed))
            .unwrap_or_default()
    }

    fn set_seed(&self, seed: ply_eval::Seed, steps: u32) {
        if let Ok(mut ctx) = self.ctx.try_borrow_mut() {
            ctx.seed = seed;
            ctx.sim_steps = steps.max(1);
        }
    }

    fn simulated(&self) -> Option<ply_eval::region::Record> {
        self.ctx
            .try_borrow()
            .ok()
            .and_then(|ctx| ctx.record.clone())
    }

    fn set_host(
        &self,
        binding: std::sync::Arc<ply_eval::HostBinding>,
        runtime: Option<std::rc::Rc<dyn ply_eval::HostRuntime>>,
    ) {
        if let Ok(mut ctx) = self.ctx.try_borrow_mut() {
            ctx.set_host(binding, runtime);
        }
    }

    fn set_declared(&self, declared: Option<ply_ty::Footprint>) {
        if let Ok(mut ctx) = self.ctx.try_borrow_mut() {
            ctx.declared = declared;
        }
    }

    fn set_re_executed(&self, re_executed: bool) {
        if let Ok(mut ctx) = self.ctx.try_borrow_mut() {
            ctx.re_executed = re_executed;
        }
    }

    fn take_host_use(&self) -> (ply_eval::host::HostUse, u64) {
        let Ok(mut ctx) = self.ctx.try_borrow_mut() else {
            return Default::default();
        };
        (
            std::mem::take(&mut ctx.host_use),
            std::mem::take(&mut ctx.host_ops),
        )
    }

    fn take_teardown(&self) -> Vec<ply_span::Diagnostic> {
        self.ctx
            .try_borrow_mut()
            .map(|mut ctx| std::mem::take(&mut ctx.teardown))
            .unwrap_or_default()
    }

    fn tier_only(&self) -> bool {
        self.tier_only
    }
}

/// Which compiled bodies the machine may enter: all, or only scalar signatures under
/// `PLY_CODEGEN_REGISTER=narrow` (read once per process).
fn registers(source: &Source, name: &str) -> bool {
    !narrow_registry() || source.scalar_signature(name)
}

pub fn narrow_registry() -> bool {
    static NARROW: OnceLock<bool> = OnceLock::new();
    *NARROW
        .get_or_init(|| std::env::var("PLY_CODEGEN_REGISTER").is_ok_and(|v| v.trim() == "narrow"))
}

/// The largest subset of `candidates` the emitter compiles as one unit, and what it dropped.
pub fn closure(source: &'static Source, candidates: &[String]) -> Result<Closed> {
    let names: Vec<&str> = candidates.iter().map(String::as_str).collect();
    let (native, refused) =
        crate::c::build(source, &names).context("compiling the fragment of this program")?;
    let set: Vec<String> = candidates
        .iter()
        .filter(|name| native.entry(name).is_some())
        .cloned()
        .collect();
    let lost = refused
        .into_iter()
        .map(|r| (r.function, r.construct))
        .collect();
    Ok((set, lost))
}

/// The surviving set, and every function that was dropped with the reason.
pub type Closed = (Vec<String>, Vec<(String, String)>);

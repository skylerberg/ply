//! The machine entering natively compiled code, from a command a user runs.

use crate::rt::Entry;
use crate::source::Source;
use anyhow::{Context, Result, bail};
use ply_eval::{
    Carry, Compilation, Counters, CtorCarries, DefHash, Diagnostic, Entered, Provider, Symbol,
    Value,
};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The widest arity this boundary carries without allocating an argument array.
const MAX_ARITY: usize = 16;

/// One admitted definition: where its code is, how many arguments it takes, what may answer an
/// entry instead of running it, and how its arguments and answer read.
struct Admitted {
    entry: Entry,
    arity: usize,
    memo: Memo,
    params: &'static [Carry],
    answer: &'static Carry,
    witnesses: &'static [usize],
}

/// The carry of a root the compiler published none for.
static UNPUBLISHED: Carry = Carry::Open;

impl Admitted {
    /// How this entry's answer reads: its type, each variable bound by what the arguments show of
    /// it. What no argument shows stays open, since only a value of that type could show it, except
    /// a witnessed variable, which [`Admitted::entered_with`] passed as `Int`.
    fn reading(&self, args: &[Value], ctors: &CtorCarries) -> Cow<'static, Carry> {
        if !self.answer.mentions_var() {
            return Cow::Borrowed(self.answer);
        }
        let mut vars = Vec::new();
        for (param, arg) in self.written().iter().zip(args) {
            param.bind(arg, &mut vars, ctors);
        }
        for &var in self.witnesses {
            if vars.len() <= var {
                vars.resize(var + 1, Carry::Open);
            }
            if vars[var] == Carry::Open {
                vars[var] = Carry::Plain;
            }
        }
        Cow::Owned(self.answer.instantiate(&vars))
    }

    /// The parameters a caller from outside passes: those after the witnesses.
    fn written(&self) -> &'static [Carry] {
        self.params.get(self.witnesses.len()..).unwrap_or(&[])
    }

    /// `args` behind the witness of each variable it is entered with: the type the arguments show
    /// for it, or `Int`'s when none does.
    fn entered_with<'a>(&self, args: &'a [Value], ctors: &CtorCarries) -> Cow<'a, [Value]> {
        if self.witnesses.is_empty() {
            return Cow::Borrowed(args);
        }
        let shown = |var: usize| {
            self.written()
                .iter()
                .zip(args)
                .find_map(|(param, arg)| param.value_at(var, arg, ctors))
                .map_or(
                    ply_eval::builtins::INT_WITNESS,
                    ply_eval::builtins::witness_of,
                )
        };
        Cow::Owned(
            self.witnesses
                .iter()
                .map(|&var| Value::Int(shown(var)))
                .chain(args.iter().cloned())
                .collect(),
        )
    }
}

/// Decided once, from the purity the compiler published: an impure root has no memo to consult.
#[derive(Clone, Copy)]
enum Memo {
    Never,
    /// A pure root of no arguments, and its memo slot.
    Constant(usize),
    /// A pure root, whose entries over memo words alone are remembered.
    Calls,
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
    /// The answer held a closure or a secret, which cannot cross out; a handle raises `E0449`.
    pub answer: u64,
    /// The answer held an `Int` word where neither its type nor the arguments say whether it is
    /// an `Int` or a width, or one that is no value of the width its type says.
    pub unread: u64,
}

impl Declines {
    pub fn total(&self) -> u64 {
        self.not_compiled
            + self.arity
            + self.reentered
            + self.touched_cells
            + self.answer
            + self.unread
    }
}

/// One run's compiled unit, shared by every worker's backend.
pub struct Unit {
    /// [`ply_eval::Front::hashes_digest`] of the program this was built over, for
    /// `Compiled::describes`.
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
        front: &ply_eval::Front,
        texts: HashMap<String, String>,
    ) -> Result<&'static Unit> {
        let identity = front.hashes_digest;
        let front: &'static ply_eval::Front = Box::leak(Box::new(front.clone()));
        let source: &'static Source =
            Box::leak(Box::new(Source::from_front(front).with_texts(texts)));
        let candidates = source.functions();
        let started = std::time::Instant::now();
        // The pre-flight decides the compiled set and leaves the unit every worker reads back.
        let (compiled, refusals) = closure(source, &candidates)?;
        let members: BTreeSet<Symbol> = compiled.iter().map(Symbol::new).collect();
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

    /// A unit the program produced, its C handed over whole: compiled and loaded once here to read
    /// the table it embeds, and loaded again by each worker that attaches it.
    pub fn handed(front: &ply_eval::Front, text: String) -> Result<&'static Unit> {
        let exports = crate::c::Exports::read(&crate::c::compile_and_load(&text, "unit")?)?;
        let identity = front.hashes_digest;
        let front: &'static ply_eval::Front = Box::leak(Box::new(front.clone()));
        let source: &'static Source = Box::leak(Box::new(Source::from_front(front)));
        let compiled = exports.names();
        let members: BTreeSet<Symbol> = compiled.iter().map(Symbol::new).collect();
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
            Some(text) => crate::c::load_unit(text, Some(self.source), "unit")?.0,
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
    fn attach(&'static self) -> Rc<dyn ply_eval::Compiled> {
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

    fn relocate(&self, front: &ply_eval::Front, sources: &ply_eval::SourceMap) -> bool {
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

/// What an entry leaves for the machine to read after it, all of it from the body it ran.
#[derive(Default)]
struct LastEntry {
    steps: u64,
    record: Option<ply_eval::region::Record>,
    performed: Vec<ply_eval::EffectAtom>,
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
    /// `run` clears it first, so an entry no body ran for reports none of the one before it.
    last: RefCell<LastEntry>,
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
            // An embedded unit's slots were chosen when it was built; this program's purity decides.
            let memo = match (
                unit.source.pure(name.as_str()),
                code.constant_index(name.as_str()),
            ) {
                (false, _) => Memo::Never,
                (true, Some(slot)) => Memo::Constant(slot),
                (true, None) => Memo::Calls,
            };
            let row = unit.source.row(name.as_str());
            admitted.insert(
                name.clone(),
                Admitted {
                    entry,
                    arity,
                    memo,
                    params: row.map_or(&[], |r| r.params.as_slice()),
                    answer: row.map_or(&UNPUBLISHED, |r| &r.answer),
                    witnesses: row.map_or(&[], |r| r.witnesses.as_slice()),
                },
            );
        }
        let mut ctx = code.context();
        ctx.program = Some(unit.source.front);
        let ctx = RefCell::new(ctx);
        Ok(Bodies {
            unit,
            _code: code,
            admitted,
            ctx,
            entered: Cell::new(0),
            declines: Cell::new(Declines::default()),
            last: RefCell::new(LastEntry::default()),
        })
    }

    /// Entries the tier answered or raised, a memo's answers among them, over this backend's
    /// whole life.
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
        *self.last.borrow_mut() = LastEntry::default();
        let Some(admitted) = self.admitted.get(name) else {
            return self.decline(|d| d.not_compiled += 1);
        };
        if admitted.arity != admitted.witnesses.len() + args.len() {
            return self.decline(|d| d.arity += 1);
        }
        let Ok(mut ctx) = self.ctx.try_borrow_mut() else {
            return self.decline(|d| d.reentered += 1);
        };
        let filled = admitted.entered_with(args, &self.unit.source.front.ctor_carries);

        let tables = std::sync::Arc::clone(&ctx.tables);
        if let Memo::Constant(index) = admitted.memo
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
        let mut all_memo = matches!(admitted.memo, Memo::Calls) && !filled.is_empty();
        for (slot, value) in handles.iter_mut().zip(filled.iter()) {
            *slot = match tables.memo_word(value) {
                Some(w) => w,
                None => {
                    all_memo = false;
                    ctx.heap.to_word(&tables.layouts, value)
                }
            };
        }
        let words = &handles[..filled.len()];
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
        if let Some(rt) = ctx.runtime.clone() {
            let warned = rt.end_entry_point(ctx.id);
            ctx.teardown.extend(warned);
        }
        *self.last.borrow_mut() = LastEntry {
            steps: u64::try_from(ctx.ticks).unwrap_or(0),
            record: ctx.record.take(),
            performed: std::mem::take(&mut ctx.performed),
        };

        if ctx.failed != 0 {
            let raised = if ctx.failed == crate::rt::FAILED_OUT_OF_FUEL {
                // Tier-only: no machine follows, so the budget is reported from here.
                Some(
                    ply_eval::Diagnostic::error(
                        ply_eval::codes::RUNTIME_ERROR,
                        format!("recursion limit of {fuel} nested calls exceeded"),
                    )
                    .primary(ctx.site(), "the call that overran it"),
                )
            } else {
                ctx.take_failure().or_else(|| {
                    (ctx.failed == crate::rt::FAILED_UNWIND).then(|| {
                        ply_eval::Diagnostic::error(
                            ply_eval::codes::RUNTIME_ERROR,
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
        crate::detached::release_all(&mut ctx);
        if !ctx.cells_balanced() {
            ctx.end();
            drop(ctx);
            return self.decline(|d| d.touched_cells += 1);
        }
        // Memoize the word and its converted value, so later entries skip both run and conversion.
        let mut walked = crate::heap::Walked::default();
        let ctors = &self.unit.source.front.ctor_carries;
        let value = crate::heap::Heap::read(
            &tables.layouts,
            out,
            &admitted.reading(args, ctors),
            ctors,
            &mut walked,
        );
        let kept = match admitted.memo {
            Memo::Constant(index) if crate::heap::world_independent(out) => {
                Some(tables.memoize(index, out))
            }
            Memo::Calls if all_memo && crate::heap::world_independent(out) => {
                tables.memoize_call(name, words, out)
            }
            _ => None,
        };
        ctx.end();
        drop(ctx);
        // Covers every handle `escape` refuses; a closure or secret that holds none declines.
        if walked.handle {
            let boundary = ply_eval::Boundary::EntryAnswer {
                name: name.as_str(),
            };
            let span = self
                .unit
                .source
                .span_of(name.as_str())
                .unwrap_or(ply_eval::Span::DUMMY);
            if let Err(refused) = ply_eval::escape::check(&boundary, &value, span) {
                self.entered.set(self.entered.get() + 1);
                return Run::Raised(refused);
            }
            return self.decline(|d| d.answer += 1);
        }
        if walked.unread {
            return self.decline(|d| d.unread += 1);
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

    fn take_performed(&self) -> Vec<ply_eval::EffectAtom> {
        std::mem::take(&mut self.last.borrow_mut().performed)
    }

    fn set_seed(&self, seed: ply_eval::Seed, steps: u32) {
        if let Ok(mut ctx) = self.ctx.try_borrow_mut() {
            ctx.seed = seed;
            ctx.sim_steps = steps.max(1);
        }
    }

    fn steps(&self) -> u64 {
        self.last.borrow().steps
    }

    fn simulated(&self) -> Option<ply_eval::region::Record> {
        self.last.borrow().record.clone()
    }

    fn set_host(
        &self,
        binding: std::sync::Arc<ply_eval::HostBinding>,
        runtime: Option<std::rc::Rc<dyn ply_eval::HostRuntime>>,
        factory: Option<ply_eval::RuntimeFactory>,
    ) {
        if let Ok(mut ctx) = self.ctx.try_borrow_mut() {
            ctx.set_host(binding, runtime, factory);
        }
    }

    fn set_declared(&self, declared: Option<ply_eval::Footprint>) {
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

    fn take_teardown(&self) -> Vec<ply_eval::Diagnostic> {
        self.ctx
            .try_borrow_mut()
            .map(|mut ctx| std::mem::take(&mut ctx.teardown))
            .unwrap_or_default()
    }
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

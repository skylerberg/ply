//! The machine entering natively compiled code, from a command a user runs.

use crate::rt::Entry;
use crate::source::Source;
use anyhow::{Context, Result, bail};
use ply_eval::{Compilation, Counters, Entered, Policed, Provider, Value};
use ply_span::{Diagnostic, Symbol};
use ply_syntax::ast::{Program, TypeExpr};
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
    /// The memo index of a pure nullary root, whose answer the seam remembers as compiled
    /// code does.
    constant: Option<usize>,
}

/// Why an offered call was not taken.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Declines {
    /// The machine offered a name this unit did not compile.
    pub not_compiled: u64,
    /// It compiled the name and the call had the wrong number of arguments.
    pub arity: u64,
    /// The body ran and failed — an overflow, a division by zero, a `match` with no arm, a type the
    /// fragment's `Int` lowering could not unbox.
    pub failed: u64,
    /// The body would have nested past the budget the machine handed it.
    pub out_of_fuel: u64,
    /// An entry arrived while another was running.
    pub reentered: u64,
    /// A builtin allocated in the fragment's private arena, which means the compile-time refusal of
    /// `cell_get`/`cell_set` has a hole in it.
    pub touched_cells: u64,
    /// The body answered a value holding a closure, a cell, a task, a continuation or a secret —
    /// nothing this boundary carries out. The machine would refuse it too; the backend refuses
    /// it first, so no registry width can leak one.
    pub answer: u64,
}

impl Declines {
    pub fn total(&self) -> u64 {
        self.not_compiled
            + self.arity
            + self.failed
            + self.out_of_fuel
            + self.reentered
            + self.touched_cells
            + self.answer
    }
}

/// One run's compiled unit: the program it answers for, the set of definitions it compiles, and
/// the counters every worker's backend adds to.
pub struct Unit {
    /// The address of the `Program` the machine is running, for `Compiled::describes`.
    origin: usize,
    source: &'static Source,
    /// The set the emitter compiles as one unit, closed under calls.
    compiled: Vec<String>,
    /// The subset of `compiled` whose whole signature is `Int` or `Bool`, which is the only part
    /// the machine can ever be offered.
    members: BTreeSet<Symbol>,
    /// Definitions the emitter refused, with the construct that refused each.
    refusals: Vec<(String, String)>,
    counters: Counters,
    /// Nanoseconds the pre-flight build took: whole-program, paid once, and the half that does
    /// **not** scale with the worker count, since every worker after it reads the unit back.
    analysis_nanos: u64,
    /// Nanoseconds workers have spent building their unit, and how many have paid it.
    codegen_nanos: AtomicU64,
    compiles: AtomicU64,
    /// Workers whose build failed after the pre-flight in [`Unit::over`] succeeded.
    poisoned: AtomicU64,
}

impl Unit {
    /// The compiled fragment of `program`, or the reason there is none.
    pub fn over(
        program: &Program,
        resolved: &ply_syntax::resolve::Resolved,
        check: &ply_core::CheckOutput,
    ) -> Result<&'static Unit> {
        Unit::keyed(program, resolved, check, HashMap::new(), HashMap::new())
    }

    /// The same, told what each definition's code is a function of, so that emitted bodies and
    /// the built unit can be kept between runs. Without the keys nothing is kept and everything
    /// is emitted afresh.
    pub fn keyed(
        program: &Program,
        resolved: &ply_syntax::resolve::Resolved,
        check: &ply_core::CheckOutput,
        keys: HashMap<String, String>,
        texts: HashMap<String, String>,
    ) -> Result<&'static Unit> {
        // The copy is what the compiled bodies are generated from, so a unit shares no state at all
        // with the machine's program.
        let origin = std::ptr::from_ref(program) as usize;
        let program: &'static Program = Box::leak(Box::new(program.clone()));
        let resolved: &'static ply_syntax::resolve::Resolved =
            Box::leak(Box::new(resolved.clone()));
        let check: &'static ply_core::CheckOutput = Box::leak(Box::new(check.clone()));
        let source: &'static Source = Box::leak(Box::new(
            Source::keyed(program, resolved, check, keys).with_texts(texts),
        ));
        let candidates = source.functions();
        let started = std::time::Instant::now();
        // The pre-flight is the analysis: the emitter's fixpoint over every function is what
        // decides the compiled set, and the unit it leaves in the cache is the one every worker
        // reads back. It is also the reason this function is fallible.
        let (compiled, refusals) = closure(source, &candidates)?;
        let members: BTreeSet<Symbol> = compiled
            .iter()
            .filter(|name| registers(source, name))
            .map(Symbol::new)
            .collect();
        let analysis_nanos = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let unit = Unit {
            origin,
            source,
            compiled,
            members,
            refusals,
            counters: Counters::default(),
            analysis_nanos,
            codegen_nanos: AtomicU64::new(0),
            compiles: AtomicU64::new(0),
            poisoned: AtomicU64::new(0),
        };
        Ok(Box::leak(Box::new(unit)))
    }

    /// The bodies this unit builds, as the concrete type rather than behind `dyn Compiled`.
    ///
    /// `attach` is the machine's door and hands back the trait object it wants. A test that is
    /// about the seam rather than about a program needs the counters, `admits` and the reentrancy
    /// hook that only `Bodies` has, and this is that door.
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
        // Offered the same set the pre-flight was, so the unit's key is the pre-flight's and a
        // worker reads that unit back rather than emitting it again.
        let candidates = self.source.functions();
        let names: Vec<&str> = candidates.iter().map(String::as_str).collect();
        let started = std::time::Instant::now();
        let (native, _refused) = crate::c::build(self.source, &names)?;
        self.codegen_nanos.fetch_add(
            u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.compiles.fetch_add(1, Ordering::Relaxed);
        Bodies::new(self, native)
    }
}

impl Provider for Unit {
    /// One worker's compiled backend.
    fn attach(&'static self, spec: &ply_eval::BackendSpec) -> Rc<dyn ply_eval::Compiled> {
        if self.members.is_empty() {
            // Nothing this worker could ever be asked, so nothing to compile.
            return ply_eval::backend::wrap(Rc::new(Absent { unit: self }), spec);
        }
        match self.build() {
            Ok(bodies) => ply_eval::backend::wrap(Rc::new(bodies), spec),
            Err(_) => {
                self.poisoned.fetch_add(1, Ordering::Relaxed);
                ply_eval::backend::wrap(Rc::new(Absent { unit: self }), spec)
            }
        }
    }

    fn name(&self) -> &'static str {
        "c"
    }

    /// The registry width, because it decides which definitions run natively at all: a pass earned
    /// under the narrow registry entered fewer of them, and is not the wide registry's pass.
    fn variant(&self) -> String {
        registry_width().to_string()
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
}

/// A worker whose compile failed: it declines everything and is counted.
struct Absent {
    unit: &'static Unit,
}

impl ply_eval::Compiled for Absent {
    fn describes(&self, program: &Program) -> bool {
        self.unit.origin == std::ptr::from_ref(program) as usize
    }

    fn enter(&self, _name: &Symbol, args: &[Value], _budget: usize) -> Option<Value> {
        self.unit.counters.note_offer(args);
        None
    }
}

impl Policed for Absent {
    fn counters(&self) -> &'static Counters {
        &self.unit.counters
    }

    fn holds(&self, _name: &Symbol) -> bool {
        false
    }

    fn answer(&self, _name: &Symbol, _args: &[Value], _budget: usize) -> Option<Value> {
        None
    }

    fn run_with_fuel(&self, _name: &Symbol, _args: &[Value], _fuel: usize) -> Option<Value> {
        None
    }
}

/// One worker's compiled bodies, offered to a `Machine` through `ply_eval::Compiled`.
pub struct Bodies {
    unit: &'static Unit,
    /// Kept alive because every [`Entry`] below points into its executable pages.
    _code: crate::c::Native,
    admitted: HashMap<Symbol, Admitted>,
    /// One context for every entry, and the `RefCell` is the proof rather than a comment:
    /// [`crate::rt::Ctx::slots`] is a bump arena with no pop, so an entry that began inside another
    /// would have to either reset it — leaving the outer activation's handles indexing different
    /// values of the same type — or let it grow for the life of the program.
    ctx: RefCell<crate::rt::Ctx>,
    entered: Cell<u64>,
    declines: Cell<Declines>,
    /// The fuel an entry was handed when the native stack refused its dive. Every offer with no
    /// more -- every call nested below that one, and a backend told to ignore its budget hands the
    /// same fuel at every depth -- is declined without entering until the machine unwinds above
    /// it, because the machine re-offers each call it evaluates and a refused dive repeated once
    /// per level is the whole recursion squared.
    floor: Cell<Option<usize>>,
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
            // A definition whose signature mentions a fixed width is compiled and called
            // directly by its neighbours, and is not offered here: compiled code holds a width as
            // the tagged immediate an `Int` is held as, so a value crossing either way would be
            // read as an `Int` where a `U32` was declared — a wrong answer rather than a slow one
            // (ADR 0039). `ply_eval`'s `Gate::ArgumentType` and `Gate::AnswerType` refuse the same
            // call on the machine's side; this is the provider declining rather than relying on
            // that, so nothing depends on which of the two runs first.
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
            floor: Cell::new(None),
        })
    }

    /// Native bodies actually run, over this backend's whole life.
    pub fn entered(&self) -> u64 {
        self.entered.get()
    }

    /// Runs `f` with this backend's context already borrowed.
    ///
    /// `Ctx` is one flat frame, so an entry that arrives while another is running would alias the
    /// outer one's words; `enter` declines on `try_borrow_mut` rather than nesting. That guard is
    /// unreachable from a program -- the machine is single-threaded and an entry does not call
    /// back into `enter` -- so this is how a test reaches it, and it exists for that.
    pub fn while_entered<T>(&self, f: impl FnOnce() -> T) -> T {
        let _held = self.ctx.borrow_mut();
        f()
    }

    /// Whether this backend would be offered `name` at all: it is registered, its signature
    /// carries, and it has a body. What the machine asks before it asks anything else.
    pub fn admits(&self, name: &str) -> bool {
        self.admitted.contains_key(&Symbol::new(name))
    }

    /// Forgets what has been entered and declined, so a test can count one call rather than a run.
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

    /// One entry, on whatever fuel the caller names.
    fn run(&self, name: &Symbol, args: &[Value], fuel: usize) -> Run {
        let Some(admitted) = self.admitted.get(name) else {
            return self.decline(|d| d.not_compiled += 1);
        };
        if admitted.arity != args.len() {
            return self.decline(|d| d.arity += 1);
        }
        if let Some(at) = self.floor.get() {
            if fuel <= at {
                return self.decline(|d| d.out_of_fuel += 1);
            }
            self.floor.set(None);
        }
        let Ok(mut ctx) = self.ctx.try_borrow_mut() else {
            return self.decline(|d| d.reentered += 1);
        };

        let tables = Rc::clone(&ctx.tables);
        // A pure nullary root already remembered is answered without running: the memo's word
        // as the value it was converted to once.
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
        // The arguments cross into the entry's own words, deep, and the answer crosses back the
        // same way below: nothing outside the entry ever holds a word.
        let mut handles = [0i64; MAX_ARITY];
        let before = ctx.heap.allocated();
        // A value this unit answered from its memo goes back in as the word it came from; a
        // call whose arguments are all such words is a pure function of remembered inputs, and
        // is remembered in turn.
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
        // SAFETY: `admitted.entry` is a pointer into `self._code`'s finalized executable pages,
        // which this struct owns and outlives the call; `ctx` is the context that unit's own
        // `Ctx::new` built, borrowed uniquely here; and `handles` is `MAX_ARITY` wide against an
        // arity this registration refused to exceed.
        let out = unsafe { (admitted.entry)(&mut *ctx as *mut crate::rt::Ctx, handles.as_ptr()) };

        if ctx.failed != 0 {
            // The fragment's diagnostic is `RUNTIME_ERROR` at `Span::DUMMY`; the machine is about
            // to evaluate the same definition and raise the real one, and a test root carries
            // this one only to name what raised when the machine then passes.
            let out_of_stack = ctx.failed == crate::rt::FAILED_OUT_OF_STACK;
            if out_of_stack {
                self.floor.set(Some(fuel));
            }
            let out_of_fuel = out_of_stack || ctx.failed == crate::rt::FAILED_OUT_OF_FUEL;
            let raised = if out_of_fuel {
                None
            } else {
                ctx.diagnostic.take()
            };
            ctx.end();
            drop(ctx);
            self.decline(|d| {
                if out_of_fuel {
                    d.out_of_fuel += 1;
                } else {
                    d.failed += 1;
                }
            });
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
        // A pure nullary root's answer is remembered as compiled code remembers it, and the
        // value it is converted to once is kept beside the word: the next entry through this
        // root answers that value without running, and the next entry handed that value passes
        // the word back in without converting it.
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
    fn describes(&self, program: &Program) -> bool {
        self.unit.origin == std::ptr::from_ref(program) as usize
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
}

impl Policed for Bodies {
    fn counters(&self) -> &'static Counters {
        &self.unit.counters
    }

    fn holds(&self, name: &Symbol) -> bool {
        self.admitted.contains_key(name)
    }

    fn answer(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
        self.run(name, args, budget).answer()
    }

    fn run_with_fuel(&self, name: &Symbol, args: &[Value], fuel: usize) -> Option<Value> {
        self.run(name, args, fuel).answer()
    }
}

/// Which of the compiled bodies are registered for the machine to enter.
///
/// Read once per process, so a test cannot set it and expect it to take now that the crate's tests
/// share one binary; measure this arm through the command.
/// Every function the fragment compiled is registered, and the seam admits each call by its
/// carried types. ADR 0030 shipped the scalar-signature registry instead, because registering
/// more only added leaf islands while the callback family was refused; with that family lowered
/// the wide registry enters at the parse root and beats no backend on the front-end row
/// (`benches/front-end`). `PLY_CODEGEN_REGISTER=narrow` keeps the arm that record measured.
fn registers(source: &Source, name: &str) -> bool {
    !narrow_registry() || scalar_signature(source, name)
}

/// Read once per process, and read here rather than at each site so that the knob has one
/// spelling: it is part of this provider's identity, which a cached result is namespaced by.
pub fn registry_width() -> &'static str {
    if narrow_registry() { "narrow" } else { "wide" }
}

pub fn narrow_registry() -> bool {
    static NARROW: OnceLock<bool> = OnceLock::new();
    *NARROW
        .get_or_init(|| std::env::var("PLY_CODEGEN_REGISTER").is_ok_and(|v| v.trim() == "narrow"))
}

/// Whether every parameter and the return type are written `Int` or `Bool`.
fn scalar_signature(source: &Source, name: &str) -> bool {
    let Some((def, _)) = source.definition(name) else {
        return false;
    };
    let scalar = |t: Option<&TypeExpr>| match t {
        Some(TypeExpr::Con { name, args, .. }) => {
            args.is_empty() && matches!(name.symbol().as_str(), "Int" | "Bool")
        }
        _ => false,
    };
    def.params.iter().all(|p| scalar(p.ty.as_ref())) && scalar(def.ret.as_ref())
}

/// The largest subset of `candidates` the emitter compiles **as one unit**, and every function
/// that was dropped with the reason.
///
/// Public so that a measurement can compile the same set the tier does. The set is the emitter's
/// fixpoint: emit everything, drop what refused, go round again, because dropping a body refuses
/// the ones that call it. Running it builds the unit, so a worker offered the same candidates
/// reads it back rather than paying it again.
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

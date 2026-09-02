//! The machine entering natively compiled code, from a command a user runs.

use crate::jit::{Entry, Jit, Opts, Unit};
use crate::source::Source;
use anyhow::{Context, Result, anyhow, bail};
use ply_eval::{Compilation, Counters, Policed, Provider, Value};
use ply_span::Symbol;
use ply_syntax::ast::{Program, TypeExpr};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet};
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

/// One run's cranelift backend: the program it answers for, the set of definitions it compiles, and
/// the counters every worker's backend adds to.
pub struct Cranelift {
    /// The address of the `Program` the machine is running, for `Compiled::describes`.
    origin: usize,
    source: &'static Source,
    /// The set the fragment compiles as one unit, closed under calls.
    compiled: Vec<String>,
    /// The subset of `compiled` whose whole signature is `Int` or `Bool`, which is the only part
    /// the machine can ever be offered.
    members: BTreeSet<Symbol>,
    /// Definitions the fragment refused, with the construct that refused each — the ranking the compute-kernel record's roadmap is read off.
    refusals: Vec<(String, String)>,
    counters: Counters,
    /// Nanoseconds the fixpoint below took: whole-program, paid once, and the half that does
    /// **not** scale with the worker count.
    analysis_nanos: u64,
    /// Nanoseconds workers have spent inside cranelift, and how many have paid it.
    codegen_nanos: AtomicU64,
    compiles: AtomicU64,
    /// Workers whose compile failed after the pre-flight in [`Cranelift::over`] succeeded.
    poisoned: AtomicU64,
}

impl Cranelift {
    /// The compiled fragment of `program`, or the reason there is none.
    pub fn over(
        program: &Program,
        resolved: &ply_syntax::resolve::Resolved,
        check: &ply_core::CheckOutput,
    ) -> Result<&'static Cranelift> {
        // The copy is what the compiled bodies are generated from, so a unit shares no state at all
        // with the machine's program.
        let origin = std::ptr::from_ref(program) as usize;
        let program: &'static Program = Box::leak(Box::new(program.clone()));
        let resolved: &'static ply_syntax::resolve::Resolved =
            Box::leak(Box::new(resolved.clone()));
        let check: &'static ply_core::CheckOutput = Box::leak(Box::new(check.clone()));
        let source: &'static Source = Box::leak(Box::new(Source::new(program, resolved, check)));
        let candidates = source.functions();
        let started = std::time::Instant::now();
        let (compiled, refusals) = closure(source, &candidates)?;
        let members: BTreeSet<Symbol> = compiled
            .iter()
            .filter(|name| registers(source, name))
            .map(Symbol::new)
            .collect();
        let analysis_nanos = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let unit = Cranelift {
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
        // The pre-flight, and it is the reason this function is fallible.
        let leaked: &'static Cranelift = Box::leak(Box::new(unit));
        if !leaked.members.is_empty() {
            leaked
                .build()
                .context("compiling the cranelift fragment of this program")?;
        }
        Ok(leaked)
    }

    /// The definitions the fragment compiles, closed under calls.
    pub fn compiled(&self) -> &[String] {
        &self.compiled
    }

    /// What the fragment refused and the construct that refused it.
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
        let names: Vec<&str> = self.compiled.iter().map(String::as_str).collect();
        let unit = Jit::compile(self.source, &names)?;
        self.codegen_nanos.fetch_add(
            u64::try_from(unit.compile_nanos).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.compiles.fetch_add(1, Ordering::Relaxed);
        Bodies::new(self, unit)
    }
}

impl Provider for Cranelift {
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
        "cranelift"
    }

    fn len(&self) -> usize {
        self.members.len()
    }

    fn offers(&self) -> ply_eval::Offers {
        self.counters.offers()
    }

    fn compilation(&self) -> Option<Compilation> {
        Some(Cranelift::compilation(self))
    }

    fn unbuilt(&self) -> u64 {
        self.poisoned()
    }
}

/// A worker whose compile failed: it declines everything and is counted.
struct Absent {
    unit: &'static Cranelift,
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
    unit: &'static Cranelift,
    /// Kept alive because every [`Entry`] below points into its executable pages.
    _code: Unit,
    admitted: HashMap<Symbol, Admitted>,
    /// One context for every entry, and the `RefCell` is the proof rather than a comment:
    /// [`crate::rt::Ctx::slots`] is a bump arena with no pop, so an entry that began inside another
    /// would have to either reset it — leaving the outer activation's handles indexing different
    /// values of the same type — or let it grow for the life of the program.
    ctx: RefCell<crate::rt::Ctx>,
    entered: Cell<u64>,
    declines: Cell<Declines>,
}

impl Bodies {
    fn new(unit: &'static Cranelift, code: Unit) -> Result<Bodies> {
        if let Some(what) = code.tables().retains_a_handle() {
            bail!(
                "the constant pool holds {what}, which must not outlive the call that made it; \
                 refusing the whole registration rather than entering anything"
            );
        }
        let mut admitted = HashMap::new();
        for name in &unit.members {
            let entry = code
                .entry(name.as_str())
                .ok_or_else(|| anyhow!("`{name}` was admitted and not compiled"))?;
            let arity = code
                .arity(name.as_str())
                .ok_or_else(|| anyhow!("`{name}` was compiled without an arity"))?;
            if arity > MAX_ARITY {
                bail!(
                    "`{name}` takes {arity} arguments and this boundary carries {MAX_ARITY}; \
                     refusing the registration rather than leaving one name that declines \
                     every call and no reason recorded against it"
                );
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
        })
    }

    /// Native bodies actually run, over this backend's whole life.
    pub fn entered(&self) -> u64 {
        self.entered.get()
    }

    pub fn declines(&self) -> Declines {
        self.declines.get()
    }

    fn decline(&self, mut f: impl FnMut(&mut Declines)) -> Option<Value> {
        let mut d = self.declines.get();
        f(&mut d);
        self.declines.set(d);
        None
    }

    /// One entry, on whatever fuel the caller names.
    fn run(&self, name: &Symbol, args: &[Value], fuel: usize) -> Option<Value> {
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
        // A pure nullary root already remembered is answered without running: the memo's word
        // as the value it was converted to once.
        if let Some(index) = admitted.constant
            && let Some(kept) = tables.memoized(index)
            && let Some(value) = tables.memo_value(kept)
        {
            drop(ctx);
            self.unit.counters.note_converted(0, 0);
            self.entered.set(self.entered.get() + 1);
            return Some(value);
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
            return Some(value);
        }
        let inward = (ctx.heap.allocated() - before) as u64;
        // SAFETY: `admitted.entry` is a pointer into `self._code`'s finalized executable pages,
        // which this struct owns and outlives the call; `ctx` is the context that unit's own
        // `Ctx::new` built, borrowed uniquely here; and `handles` is `MAX_ARITY` wide against an
        // arity this registration refused to exceed.
        let out = unsafe { (admitted.entry)(&mut *ctx as *mut crate::rt::Ctx, handles.as_ptr()) };

        if ctx.failed != 0 {
            // The fragment's own diagnostic is deliberately dropped on the floor: it is
            // `RUNTIME_ERROR` at `Span::DUMMY`, and the machine is about to evaluate the same
            // definition and raise the real one.
            let out_of_fuel = ctx.failed == crate::rt::FAILED_OUT_OF_FUEL;
            ctx.end();
            drop(ctx);
            return self.decline(|d| {
                if out_of_fuel {
                    d.out_of_fuel += 1;
                } else {
                    d.failed += 1;
                }
            });
        }
        if ctx.touched_cells() {
            ctx.end();
            drop(ctx);
            return self.decline(|d| d.touched_cells += 1);
        }
        // A pure nullary root's answer is remembered as compiled code remembers it, and the
        // value it is converted to once is kept beside the word: the next entry through this
        // root answers that value without running, and the next entry handed that value passes
        // the word back in without converting it.
        let mut outward = 0;
        let value = crate::heap::Heap::to_value_counted(&tables.layouts, out, &mut outward);
        let kept = match admitted.constant {
            Some(index) if crate::heap::world_independent(out) => Some(tables.memoize(index, out)),
            None if all_memo && crate::heap::world_independent(out) => {
                tables.memoize_call(name, words, out)
            }
            _ => None,
        };
        ctx.end();
        drop(ctx);
        if crate::rt::holds_a_handle(&value).is_some() {
            return self.decline(|d| d.answer += 1);
        }
        if let Some(kept) = kept {
            tables.remember(kept, &value);
        }
        self.unit.counters.note_converted(inward, outward);
        self.entered.set(self.entered.get() + 1);
        Some(value)
    }
}

impl ply_eval::Compiled for Bodies {
    fn describes(&self, program: &Program) -> bool {
        self.unit.origin == std::ptr::from_ref(program) as usize
    }

    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
        self.unit.counters.note_offer(args);
        self.run(name, args, budget)
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
        self.run(name, args, budget)
    }

    fn run_with_fuel(&self, name: &Symbol, args: &[Value], fuel: usize) -> Option<Value> {
        self.run(name, args, fuel)
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
    static NARROW: OnceLock<bool> = OnceLock::new();
    let narrow = *NARROW
        .get_or_init(|| std::env::var("PLY_CODEGEN_REGISTER").is_ok_and(|v| v.trim() == "narrow"));
    !narrow || scalar_signature(source, name)
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

/// The largest subset of `candidates` the fragment compiles **as one unit**, and every function
/// that was dropped with the reason.
fn closure(source: &'static Source, candidates: &[String]) -> Result<Closed> {
    let mut set: Vec<String> = candidates.to_vec();
    let mut lost: Vec<(String, String)> = Vec::new();
    loop {
        if set.is_empty() {
            break;
        }
        let names: Vec<&str> = set.iter().map(|s| s.as_str()).collect();
        let refusals = Jit::refusals(source, &names, Opts::default())?;
        if refusals.is_empty() {
            break;
        }
        let refused: HashSet<&str> = refusals.iter().map(|r| r.function.as_str()).collect();
        for r in &refusals {
            lost.push((r.function.clone(), r.construct.clone()));
        }
        let before = set.len();
        set.retain(|n| !refused.contains(n.as_str()));
        if set.len() == before {
            bail!(
                "the fragment refused {} function(s) and named none of the ones it was given: {:?}",
                refusals.len(),
                refusals
                    .iter()
                    .map(|r| r.function.as_str())
                    .collect::<Vec<_>>()
            );
        }
    }
    Ok((set, lost))
}

/// The surviving set, and every function that was dropped with the reason.
type Closed = (Vec<String>, Vec<(String, String)>);

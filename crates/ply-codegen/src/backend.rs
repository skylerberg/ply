//! The machine entering natively compiled code, from a command a user runs.
//!
//! `crates/ply-codegen-spike/src/entry.rs` is where this comes from, and ADR
//! 0018 §0 is why that file existed: the fragment reached **52.58×** where it
//! ran and **0.998×** end to end, because calls only ever went one way. A
//! function the fragment accepted whose *callers* it refused was compiled and
//! then never entered.
//!
//! What is new here is not the code generator. It is that
//! [`ply_eval::Compiled`] now has a second implementor on the **shipping** side
//! of the seam, so `ply test --backend cranelift` installs one and the eight
//! deliberately wrong backends of `ply_eval::backend` wrap it. ADR 0026 §4.5
//! made that the condition on any backend shipping at all — *"a backend must be
//! policeable before it is fast"* — and it was discharged for exactly one
//! backend until this crate existed.
//!
//! # What the machine is promised, and who enforces it
//!
//! `Compiled::enter` hands over a name, some scalars and a call budget, and
//! takes back at most one scalar. It is handed no arena, no stack, no handler
//! stack, no host binding, no `&mut Machine` and no callback — so the promises
//! below are kept by there being no route to break them, not by this file
//! remembering to:
//!
//! | promise | kept by |
//! | --- | --- |
//! | a native body reaches no Ply function outside the compiled set | [`crate::jit::Denotes::Uncompiled`] refuses the caller at compile time |
//! | it performs no effect and captures no continuation | there is no machine to perform into, and `perform`/`handle` are outside the fragment |
//! | it touches no cell and opens no region | `cell_get`/`cell_set` refused at compile time; [`crate::rt::Ctx::touched_cells`] is the armed check |
//! | it calls no user code from a builtin | `Builtin::higher_order` refused at compile time |
//! | it cannot outrun `ply_eval::limit` | the fuel prologue, seeded from `budget` |
//! | it raises nothing | a failure answers `None` and the machine raises its own diagnostic |
//! | it cannot be entered from inside itself | one `Ctx` behind a `RefCell`, which declines rather than resetting |
//!
//! The one thing **not** structural, and it is worth saying plainly: a compiled
//! body that computes the wrong `Int` is a wrong answer this boundary cannot
//! detect. `ply test --engine both` comparing the backed machine against the
//! plain one is what catches that, and the eight corruptions are what say the
//! comparison works. Nothing here does.

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
/// A wider function declines; nothing in the standard library is near it, and a
/// heap allocation per native call would price the hook rather than the code.
const MAX_ARITY: usize = 16;

/// One admitted definition: where its code is, and how many arguments it takes.
struct Admitted {
    entry: Entry,
    arity: usize,
}

/// Why an offered call was not taken.
///
/// R4's null result was a speedup reported with zero entries and nothing in the
/// harness that could say so, so a decline is counted by its reason rather than
/// counted at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Declines {
    /// The machine offered a name this unit did not compile.
    pub not_compiled: u64,
    /// It compiled the name and the call had the wrong number of arguments.
    pub arity: u64,
    /// The body ran and failed — an overflow, a division by zero, a `match`
    /// with no arm, a type the fragment's `Int` lowering could not unbox.
    pub failed: u64,
    /// The body would have nested past the budget the machine handed it. The
    /// machine then re-evaluates and raises its own bound.
    pub out_of_fuel: u64,
    /// An entry arrived while another was running. Structurally impossible —
    /// nothing a compiled body can call re-enters a machine — and counted
    /// rather than asserted, because the day it stops being impossible the
    /// count is the only thing that would say so.
    pub reentered: u64,
    /// A builtin allocated in the fragment's private arena, which means the
    /// compile-time refusal of `cell_get`/`cell_set` has a hole in it.
    pub touched_cells: u64,
}

impl Declines {
    pub fn total(&self) -> u64 {
        self.not_compiled
            + self.arity
            + self.failed
            + self.out_of_fuel
            + self.reentered
            + self.touched_cells
    }
}

/// One run's cranelift backend: the program it answers for, the set of
/// definitions it compiles, and the counters every worker's backend adds to.
///
/// Leaked on construction for the reason `ply_eval::Fragment` is: a backend may
/// not borrow the program the machine is running.
///
/// # Why the compile happens per worker and not once here
///
/// [`Provider::attach`] runs on the worker's own thread and JIT-compiles this
/// unit's set there. That is N compiles for N workers, and it is not an
/// oversight:
///
/// - A compiled unit owns a `JITModule` and a constant pool of [`Value`]s, and a
///   `Value` is `Rc` all the way down — `ply_eval::Machine::set_compiled` takes
///   an `Rc<dyn Compiled>` for that reason. There is no sharing a unit across
///   threads without a second representation of every constant.
/// - The alternative that does not need one is a mutex around a single
///   `Ctx`, which would serialise every native call in a run whose whole
///   purpose is that native calls are cheap.
///
/// What the cost of that decision is, on this repository's own corpora, is
/// measured rather than assumed: see [`Cranelift::compile_nanos`] and ADR 0026
/// §4.9.
///
/// The **fixpoint** — which definitions can be compiled as one closed set — is
/// computed exactly once, here, because it is a property of the program rather
/// than of a thread, and it is the expensive half.
pub struct Cranelift {
    /// The address of the `Program` the machine is running, for
    /// `Compiled::describes`. Never dereferenced — a `usize` rather than a
    /// `*const Program` so this type stays `Sync`.
    origin: usize,
    source: &'static Source,
    /// The set the fragment compiles as one unit, closed under calls.
    compiled: Vec<String>,
    /// The subset of `compiled` whose whole signature is `Int` or `Bool`, which
    /// is the only part the machine can ever be offered.
    members: BTreeSet<Symbol>,
    /// Definitions the fragment refused, with the construct that refused each —
    /// the ranking ADR 0018 §0's roadmap is read off.
    refusals: Vec<(String, String)>,
    counters: Counters,
    /// Nanoseconds the fixpoint below took: whole-program, paid once, and the
    /// half that does **not** scale with the worker count.
    ///
    /// It is the *fixpoint* and not the whole of [`Cranelift::over`]: the leaked
    /// copies of the program, its resolution and its check output are outside
    /// the window, because `ply_eval::Fragment::over` makes the identical copies
    /// and a run with `--backend reference` pays them too. So this number is
    /// what choosing a code generator costs over choosing a tree-walker, which
    /// is the comparison a reader of the report is making.
    analysis_nanos: u64,
    /// Nanoseconds workers have spent inside cranelift, and how many have paid
    /// it. A sum and a count rather than a mean, so a reader can divide and a
    /// run that installed one backend is distinguishable from one that
    /// installed ten.
    codegen_nanos: AtomicU64,
    compiles: AtomicU64,
    /// Workers whose compile failed after the pre-flight in [`Cranelift::over`]
    /// succeeded.
    ///
    /// Always zero, and it may not be silent if it ever is not: a worker with no
    /// backend would decline every call and the run would be green over a seam
    /// nobody reached, which is `CONTRIBUTING.md` §"The one rule"'s defect shape
    /// exactly. `ply test` turns a non-zero count here into an `INTERNAL_ERROR`
    /// and fails the run.
    ///
    /// **Watched to fail rather than reasoned about**, because a counter that
    /// can never move is the kind of armed-looking check this project has been
    /// bitten by. Making [`Cranelift::build`] `bail!` on every call after the
    /// pre-flight, `ply test --backend cranelift` reports *"error: 1 worker(s)
    /// could not build the `cranelift` backend, and every call they were offered
    /// was declined"* and exits **1** — over a corpus whose five tests all
    /// passed, which is the point: the verdict was green and the run is not.
    /// Restored, the same command exits 0.
    poisoned: AtomicU64,
}

impl Cranelift {
    /// The compiled fragment of `program`, or the reason there is none.
    ///
    /// Fallible on purpose and called from the command rather than from a
    /// worker: a host with no cranelift backend, or a program the fixpoint
    /// cannot close, is a thing a user must be *told*, and the only place a
    /// diagnostic can still be raised is before the run starts.
    pub fn over(
        program: &Program,
        resolved: &ply_syntax::resolve::Resolved,
        check: &ply_core::CheckOutput,
    ) -> Result<&'static Cranelift> {
        // The copy is what the compiled bodies are generated from, so a unit
        // shares no state at all with the machine's program. `describes` still
        // compares against the *original* address, exactly as
        // `ply_eval::Fragment::over` does, so a bisection that builds a program
        // whose definitions carry the names of the ones they replace gets a
        // backend that declines to describe it.
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
        // The pre-flight, and it is the reason this function is fallible. It
        // compiles the set once on the calling thread and throws the result
        // away, so that "this host has no Cranelift backend" and "the constant
        // pool retains a handle" are diagnostics a user reads rather than a
        // worker that quietly declines everything.
        //
        // Skipped when the fragment is empty, because then no worker will ever
        // compile either: with no enterable member the machine can never be
        // answered, and compiling a closed set nothing can enter would be work
        // whose only product is a number in a report.
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

    /// Workers whose compile failed. Always zero; see the field.
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
    ///
    /// A compile that fails here has already succeeded once in
    /// [`Cranelift::over`], on this host and this program, so this arm is
    /// unreachable rather than merely unlikely. It is counted rather than
    /// ignored, and `ply test` fails the run on a non-zero count: a worker with
    /// no bodies declines every call, which would leave a **green** run over a
    /// seam nothing reached.
    fn attach(&'static self, spec: &ply_eval::BackendSpec) -> Rc<dyn ply_eval::Compiled> {
        if self.members.is_empty() {
            // Nothing this worker could ever be asked, so nothing to compile.
            // Not a failure and not counted as one: `fragment 0` in the report
            // is what says the fragment reached nothing, and it says it whether
            // or not a code generator ran.
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
///
/// Not a fallback and not a degradation. It exists because
/// [`Provider::attach`] cannot fail and a run that pretended to have a backend
/// must still be able to say it did not — [`Cranelift::poisoned`] is what says
/// it, and `ply test` fails on it.
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

/// One worker's compiled bodies, offered to a `Machine` through
/// `ply_eval::Compiled`.
pub struct Bodies {
    unit: &'static Cranelift,
    /// Kept alive because every [`Entry`] below points into its executable
    /// pages. Dropping it while an `Admitted` is live would be a dangling
    /// function pointer, which is why it is a field and not a temporary.
    _code: Unit,
    admitted: HashMap<Symbol, Admitted>,
    /// One context for every entry, and the `RefCell` is the proof rather than
    /// a comment: [`crate::rt::Ctx::slots`] is a bump arena with no pop, so an
    /// entry that began inside another would have to either reset it — leaving
    /// the outer activation's handles indexing different values of the same
    /// type — or let it grow for the life of the program. Both are silent.
    /// Declining is neither.
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
            admitted.insert(name.clone(), Admitted { entry, arity });
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
    ///
    /// The number M9 exists to move. A ratio reported beside a zero here is a
    /// null result whatever it says.
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
    ///
    /// `fuel` is the machine's remaining budget on the honest path and is
    /// deliberately something else on [`ply_eval::Mutation::ExceedsBudget`]'s;
    /// the two share this body so that the corruption is one number and not a
    /// second code path.
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

        ctx.begin(i64::try_from(fuel).unwrap_or(i64::MAX));
        let mut handles = [0i64; MAX_ARITY];
        for (slot, value) in handles.iter_mut().zip(args) {
            *slot = ctx.push(value.clone());
        }
        // SAFETY: `admitted.entry` is a pointer into `self._code`'s finalized
        // executable pages, which this struct owns and outlives the call;
        // `ctx` is the context that unit's own `Ctx::new` built, borrowed
        // uniquely here; and `handles` is `MAX_ARITY` wide against an arity
        // this registration refused to exceed.
        let out = unsafe { (admitted.entry)(&mut *ctx as *mut crate::rt::Ctx, handles.as_ptr()) };

        if ctx.failed != 0 {
            // The fragment's own diagnostic is deliberately dropped on the
            // floor: it is `RUNTIME_ERROR` at `Span::DUMMY`, and the machine is
            // about to evaluate the same definition and raise the real one.
            // Which failure it was is still counted, because "the budget ran
            // out" and "the arithmetic overflowed" are different facts about a
            // run.
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
        let value = ctx.read(out).clone();
        ctx.end();
        drop(ctx);
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

    /// Whether a body was registered for `name`.
    ///
    /// What separates "the fragment ran this and declined" from "this name was
    /// never given a body at all" — the distinction
    /// [`ply_eval::Mutation::Unoffered`] lives in.
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
/// Measurement scaffolding, off unless `PLY_CODEGEN_REGISTER=all` is set in the
/// environment, and **read once per process** -- `ply_eval::backend`'s
/// `PLY_BACKEND_ONLY` is the precedent and this follows its shape deliberately.
///
/// It exists to settle a *cost* claim rather than a correctness one.
/// [`scalar_signature`] drops 467 of the 489 bodies the fixpoint compiles over
/// `spikes/ply-parser`, and its stated reason is that declining before the fact
/// is cheaper than declining at runtime. That is a claim about time, and the
/// only way to know is to register them and take the clock.
///
/// Widening is **not** a correctness risk, which is why this knob is safe to
/// hand a corpus: the runtime unboxes dynamically, so `rt_unbox_int` on a
/// `Bytes` calls `ctx.fail` and the entry declines with `Declines::failed`
/// counting it. `scalar_signature`'s own doc says the machine's boundary is the
/// authority on both sides. What widening can change is how much is *paid* per
/// declined call, not what any call answers.
///
/// Deliberately not a `--backend` spec, for `PLY_BACKEND_ONLY`'s reason: a spec
/// is a user-facing promise and this is an instrument.
fn registers(source: &Source, name: &str) -> bool {
    static ALL: OnceLock<bool> = OnceLock::new();
    let all =
        *ALL.get_or_init(|| std::env::var("PLY_CODEGEN_REGISTER").is_ok_and(|v| v.trim() == "all"));
    all || scalar_signature(source, name)
}

/// Whether every parameter and the return type are written `Int` or `Bool`.
///
/// Necessary and not sufficient, and the machine's boundary is the authority on
/// both sides anyway. It is here so that a function which would *always*
/// decline is never registered: `std.http.read_line` takes `Bytes`, and a
/// `Float` has no path in this fragment and compiles as `Int` arithmetic
/// regardless (ADR 0019 §5 item 4). Declining before the fact is cheaper than
/// declining 120,000 times, and a refusal that fires constantly is a bug report
/// rather than a fast path.
///
/// **Narrower than `ply_eval::compiled::crossable`, on purpose.** The seam
/// carries `Bytes` as of 2026-08-30 and `ply_eval::backend::Reference` answers
/// for it; this fragment has no `Bytes` path at all, so a `Bytes` signature
/// here would be a body that unboxes an `Int` from an `Arc<[u8]>` and fails
/// every time. The two registries have parted company and the wider one is the
/// tree-walker's.
///
/// > **Measured, 2026-08-31 -- ADR 0032.** The cost claim above is true and the
/// > margin is larger than it reads. This filter drops **467 of the 489** bodies
/// > the fixpoint compiles over `spikes/ply-parser`; registering them all
/// > (`PLY_CODEGEN_REGISTER=all`) takes the front end from 3.04 s to 4.72 s
/// > against a 2.85 s interpreter, because it buys 495,152 shallow entries
/// > rather than a higher one. On `benches/kernel` the same widening is worth
/// > 1.5x, because there the root *is* compilable and 2,974 entries collapse to
/// > 63. Keep the narrow default; the way past it is compiling `++` and nested
/// > record patterns, not registering more leaves.
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

/// The largest subset of `candidates` the fragment compiles **as one unit**,
/// and every function that was dropped with the reason.
///
/// Not a list somebody read off a census: a name survives only if every
/// construct in its body is inside the fragment *and* every Ply function it can
/// reach is also in the set. Dropping one function refuses its callers on the
/// next round, so this is a fixpoint rather than a filter, and it terminates
/// because every round that changes anything removes at least one name.
///
/// The result is what makes the promise [`Bodies`] gives the machine true by
/// construction: from inside a member there is no reachable call that leaves
/// compiled code.
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

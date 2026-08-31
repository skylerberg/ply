//! The control-stack machine.
//!
//! A configuration is `⟨S, K, W⟩` — state, [`Stack`], [`TaskRegions`] — and [`step`]
//! is one transition of ADR 0005 §1.3. Nothing about a Ply computation lives on
//! the native stack: a call costs one [`Frame::Call`] on the heap, which is what
//! makes capturing a continuation O(one segment per enclosing handler) and what
//! turns the old depth guard into an exact, O(1) bound on pending frames.
//!
//! [`step`]: Machine::step

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
use crate::interp::{
    OpTable, arity_error, ctor_value, err_non_exhaustive, err_not_a_function, err_overflow,
    err_unknown_name, op_decl,
};
use crate::limit::{self, DEFAULT_MAX_CALLS, NAMED_CALLS, NESTED_CALLS};
use crate::memo::{Lookup, Memo};
use crate::rc::Own;
use crate::region::{self, Region, StepSite, Trail};
use crate::sched::{HostPolicy, Policy, Resumption, Scheduler, Turn};
use crate::sim::{Access, Answer, DEFAULT_STEPS, Seed};
use crate::task_regions::TaskRegions;
use crate::trace::Trace;
use crate::value::{Closure, ClosureKind, Value};
use ply_core::CheckOutput;
use ply_core::ty::{EffectAtom, Footprint};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{
    BinOp, Expr, FnDef, Item, Mode, Pattern, PatternKind, Program, QName, TypeDefBody, UnOp,
};
use ply_syntax::resolve::{Namespace, Resolved};
use rustc_hash::FxHashMap;
use std::cell::{Cell, OnceCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

// `pub const DEFAULT_MAX_FRAMES: usize = 1_000_000` stood here and was this
// machine's default frame ceiling. It is gone; `Machine::with_max_frames`
// carries what it used to say and why it had to go.
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
    /// This machine's identity, which with the performing task is what a host
    /// handler keys scoped state on. Minted per machine because `ply test` drives
    /// one per worker thread and every one of them performs outside a scheduler
    /// region — so the task alone files them all under a single owner.
    id: MachineId,
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
    /// Where the lowering itself is kept, and the reason `lowered` above is only
    /// a map from a name to a closure this machine has already built.
    ///
    /// Scoped to the *program*, for the reason `region_kinds` below is: what a
    /// body lowers to is a property of the syntax and of nothing a machine
    /// holds, so a machine built next over the same program
    /// ([`Machine::share_lowering`]) reads what this one lowered rather than
    /// lowering it again. That is what a search costs otherwise — it builds a
    /// machine per interleaving.
    lowering: Rc<Lowering<'a>>,
    /// The lowered body of the last tree-walker closure this machine applied.
    /// Only [`Interp`] and the prover's generators build one, so this is empty
    /// in a run of the machine alone.
    ///
    /// [`Interp`]: crate::Interp
    closure_code: ClosureCode,
    /// What a nullary pure definition evaluated to, so a service does not
    /// rebuild its route table once per request.
    memo: Memo,
    ctors: FxHashMap<Symbol, usize>,
    ops: OpTable,
    tests: Vec<TestSlot<'a>>,
    /// Where every cell this engine allocates lives, and the fixture every
    /// entry point resets to — so one seeded fixture serves every test in a run
    /// without any of them observing another's writes. ADR 0017 §1 and §5.
    regions: TaskRegions,
    /// Which of ADR 0017 §3's two kinds each region in this program is.
    ///
    /// Lazily, for the reason `lowered` is lazy: a program whose entry point
    /// opens no region must not pay for a whole-program analysis. `/health`
    /// opens none.
    ///
    /// Scoped to the *program* rather than to this machine, because that is
    /// what the answer is a property of. A machine built from a program another
    /// machine has already analysed is handed that analysis
    /// ([`Machine::share_region_kinds`]) instead of repeating it; a machine
    /// handed none analyses its own program on first need, which is correct and
    /// is what a machine per entry point used to cost.
    region_kinds: crate::region_kind::Kinds,
    /// What this entry point performed, which is not what its row said it could.
    trace: Trace,
    stack: Stack,
    state: State,
    current: Span,
    /// An opt-in ceiling on this engine's own heap. `None` — the default — is
    /// what keeps a program's answer a function of the program: see
    /// [`Machine::with_max_frames`].
    max_frames: Option<usize>,
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
    /// A source of natively compiled bodies. `None` on every machine this
    /// workspace builds — nothing in `crates/*` implements [`Compiled`], and the
    /// doubles in this crate's tests are the only implementors that ship.
    ///
    /// `'static` rather than `'a`, and it has to be: a trait object is opaque to
    /// dropck, so a `dyn Compiled + 'a` here would make every `Machine<'a>` count
    /// as using `'a` when it drops — which turns the ordinary
    /// `Machine::new(&c.program, ..)` followed by `c` moving out into a borrow
    /// error at every existing call site. A backend therefore may not borrow the
    /// program; [`Compiled::describes`] is a pointer comparison, which a stored
    /// `*const Program` serves without a borrow.
    ///
    /// [`Compiled`]: crate::Compiled
    /// [`Compiled::describes`]: crate::Compiled::describes
    compiled: Option<Rc<dyn crate::Compiled>>,
    /// Native entries taken, calls a backend was offered and declined, and
    /// answers refused at the boundary. Cumulative over the machine's life
    /// rather than per entry point, like `teardown` above.
    ///
    /// [`Cell`] rather than plain fields so `Machine::compiled_answer` can stay
    /// `&self` — which is what makes a decline provably free, since a method that
    /// cannot mutate the machine cannot have committed anything to undo.
    ///
    /// A measurement reporting a speedup with `entries == 0` is reporting a null
    /// result. R4's 0.998x was exactly that and nothing in the harness said so.
    compiled_entries: Cell<u64>,
    compiled_declines: Cell<u64>,
    compiled_refusals: Cell<u64>,
    /// Which definitions' declared parameter types cannot reach a world handle
    /// — `compiled::Gate::ArgumentType`'s table.
    ///
    /// Lazily, for the reason `lowered` and `region_kinds` are lazy: building it
    /// is a pass over every constructor and every definition in the program, and
    /// a machine that never offers a call must not pay for one. `OnceCell`
    /// rather than a field so `compiled_answer` and `census_call` can stay
    /// `&self`, which is what makes a decline provably free.
    ///
    /// Scoped to this machine rather than to the program, unlike `lowering`: it
    /// is a property of the `CheckOutput`, and a machine handed none builds an
    /// empty table that admits nothing.
    carried_types: OnceCell<crate::compiled::CarriedTypes>,
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
    /// What the runtime reported while closing entry points. Warnings, never
    /// failures, and cumulative across the machine's life rather than per entry
    /// point: a connection discarded by the third test is still a fact about the
    /// run when the tenth finishes.
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

    /// Caps this engine's own heap at `max` pending frames. Off unless asked
    /// for, and **not** part of what a program means.
    ///
    /// The frames are heap cells the machine keeps because nothing about a Ply
    /// computation lives on the native stack. The tree-walker spends the native
    /// stack on the same nesting and has no counterpart, so a ceiling here is
    /// one engine's resource guard and nothing a program's answer may turn on.
    /// Setting it is for a test that wants to observe the stack's shape, or for
    /// an embedder that would rather have a diagnostic than an OOM kill —
    /// knowing that the value it picks is a limit only this engine enforces.
    /// A machine that has one enters no compiled body, because a native body
    /// pends no frames and so cannot honour it (`compiled_answer`).
    ///
    /// > **Corrected (2026-08-24): this used to be a default, `DEFAULT_MAX_FRAMES
    /// > = 1_000_000`, and its exhaustion used to be a program-level `recursion
    /// > limit` diagnostic.** The constant's doc read *"A bound on pending
    /// > frames: a resource limit, not a native-stack workaround. The frames are
    /// > heap cells and this is how many of them a program may hold at once. It
    /// > catches a program that pends a million frames without nesting ten
    /// > thousand calls."* An R5 review then corrected it in place with *"A
    /// > **call** costs one frame; a **body** costs as many as it pends. A
    /// > recursion whose body pends `k` frames per level reaches **this** bound
    /// > first whenever `k > DEFAULT_MAX_FRAMES / DEFAULT_MAX_CALLS` = 100"*,
    /// > and named two consequences it left open: the two engines disagreed on
    /// > such a program with no backend involved, and the compiled-entry seam
    /// > could not express the bound at all.
    /// >
    /// > What settles it is that the bound was a function of **spelling**, not
    /// > of behaviour. Measured with the shipping `target/release/ply` on
    /// > 2026-08-24 — two definitions of the same function `hog(n) = 150n`,
    /// > making the same 9,001 nested calls, at `hog(9000)`:
    /// >
    /// > ```text
    /// > hog(n - 1) + 150            ok    one addition of 150
    /// > hog(n - 1) + 1 + 1 + ...    FAIL  recursion limit of 1000000 pending frames exceeded
    /// > ```
    /// >
    /// > A semantic bound may not separate those. The frame count also protects
    /// > nothing at program level that the product does not already spend. Peak
    /// > RSS, `/usr/bin/time -l`, debug, one process per figure: the machine
    /// > holds 1,350,000 pending frames in **194 MiB**, about **151 bytes** a
    /// > frame; the tree-walker holds the same 1,350,000 levels of the same
    /// > program in **5,365 MiB**, about **4.2 KiB** a level, and reports
    /// > `passed`. The engine carrying the guard was the one spending about a
    /// > **28th** as much of the resource it guarded.
    /// > `CONTRIBUTING.md` §"Things known to be broken" items 9 and 10.
    pub fn with_max_frames(mut self, max: usize) -> Machine<'a> {
        self.max_frames = Some(max.max(1));
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

    pub fn cells(&self) -> &Arena {
        self.regions.arena()
    }

    pub fn cells_mut(&mut self) -> &mut Arena {
        self.regions.arena_mut()
    }

    pub fn regions(&self) -> &TaskRegions {
        &self.regions
    }

    /// The kind of the region opened at `span`, and `None` when that span opens
    /// no region: one the inference never saw, or a `with_cell[r]` nested inside
    /// the `with_region[r]` that already opened `r`.
    pub fn region_kind(&self, span: Span) -> Option<RegionKind> {
        self.region_kinds().at(span).map(|region| region.kind)
    }

    /// This program's region kinds, inferring them if nothing has yet.
    pub fn region_kinds(&self) -> &crate::region_kind::Regions {
        self.region_kinds
            .get_or_init(|| crate::region_kind::infer(self.program, self.resolved))
    }

    /// The handle to hand another engine built from **this same program**, so
    /// the analysis behind it runs once for the program rather than once per
    /// engine.
    pub fn shared_region_kinds(&self) -> crate::region_kind::Kinds {
        crate::region_kind::Kinds::clone(&self.region_kinds)
    }

    /// Take another engine's answer for this program instead of inferring one.
    ///
    /// `kinds` must have been filled — or be about to be filled — from the same
    /// `(Program, Resolved)` pair this machine holds. A handle from a different
    /// program is an answer about the wrong program, and where the two agree on
    /// a span it can say `unique` for a region the running program captures
    /// across, which frees an arena a continuation still reaches.
    pub fn share_region_kinds(&mut self, kinds: crate::region_kind::Kinds) {
        self.region_kinds = kinds;
    }

    /// The lowering cache to hand a machine built next over **this same
    /// program**, so a body is lowered once for the program rather than once per
    /// machine.
    ///
    /// Not a [`crate::region_kind::Kinds`]-shaped handle, and it cannot be one:
    /// lowered code is `Rc`, so this is shareable between machines on one thread
    /// and between no two threads.
    pub fn share_lowering(&self) -> Rc<Lowering<'a>> {
        Rc::clone(&self.lowering)
    }

    /// Lower into `lowering` rather than into a cache of this machine's own.
    ///
    /// **Ignored when `lowering` was taken over a different program**, where it
    /// would answer nothing this machine asks it. A bisection builds a program
    /// whose definitions carry the names of the ones they replace, and the
    /// nearest thing to a caller passing the wrong cache is there.
    pub fn set_lowering(&mut self, lowering: Rc<Lowering<'a>>) {
        if lowering.describes(self.program) {
            self.lowering = lowering;
        }
    }

    /// Take compiled bodies for this program's definitions, so a call the
    /// backend accepts is entered natively instead of evaluated.
    ///
    /// **Ignored when `compiled` was built over a different program**, exactly as
    /// [`Machine::set_lowering`] is and for the same reason: a bisection builds a
    /// program whose definitions carry the names of the ones they replace, and a
    /// registry keyed on a bare name would answer for the wrong body.
    ///
    /// Nothing in this workspace implements [`Compiled`] and no shipping command
    /// calls this. Two consequences, both stated rather than guarded:
    ///
    /// - A run with a backend attached is a third execution strategy, and a
    ///   cached `Pass` is a claim about the authoritative engine
    ///   ([`crate::EngineChoice::bypasses_cache`]). That rule is **not enforced
    ///   for a backend** — it is unreachable, because `cache_bypassed` at
    ///   `crates/ply-cli/src/commands/test.rs:335` takes a `&TestArgs` with no
    ///   `Machine` in scope and no shipping command can install one. The day a
    ///   flag can, that line moves in the same change.
    /// - A backend's answer is trusted to be the definition's. The boundary
    ///   checks its *kind* and nothing else; a wrong `Int` is caught by
    ///   `--engine both` and by nothing here.
    ///
    /// > **The first paragraph and the first bullet are withdrawn, 2026-08-28.
    /// > That day came, and the line moved in the same change.** They read:
    /// > *"Nothing in this workspace implements [`Compiled`] and no shipping
    /// > command calls this"*, and *"That rule is **not enforced for a backend**
    /// > — it is unreachable … The day a flag can, that line moves in the same
    /// > change."*
    /// >
    /// > [`crate::backend::Reference`] implements it and
    /// > `ply test --backend <spec>` installs it, through the one production
    /// > caller this method has: `ply_test::InterpExecutor::machine_lowering`.
    /// > The rule is armed twice, per ADR 0026 §4.6, because one of the halves
    /// > alone would be an accident — `--engine both` already bypasses the cache,
    /// > so a backend on *that* path would be safe for a reason that has nothing
    /// > to do with backends:
    /// >
    /// > - `cache_bypassed` reads `args.backend`, so a backend run on the
    /// >   **default** engine reads nothing from the store either. That is the
    /// >   flag half and it covers a backend that arrives by the flag.
    /// > - `ply_test::run_with` records `ply_test::Record::Backend` for any
    /// >   test whose native entry count is non-zero, so nothing is written
    /// >   whatever the flags said, and `ply test`'s `backend_escapes` turns a
    /// >   written `Pass` beside a non-zero count into an `INTERNAL_ERROR`. That
    /// >   is the half that survives a backend arriving by a route no flag names.
    /// >
    /// > The second bullet stands unchanged and is now checked rather than
    /// > stated: `--engine both --backend wrong:off-by-one` is caught by two
    /// > tests on the value axis, and six of the other seven configurations are
    /// > caught too. `crates/ply-cli/tests/backend.rs` names the eighth.
    ///
    /// [`Compiled`]: crate::Compiled
    pub fn set_compiled(&mut self, compiled: Rc<dyn crate::Compiled>) {
        if compiled.describes(self.program) {
            self.compiled = Some(compiled);
        }
    }

    /// Native entries taken and calls declined, over this machine's whole life.
    ///
    /// For a harness that must not report a ratio without them: a speedup
    /// measured with `entries == 0` is a null result, which is what R4's 0.998x
    /// was.
    pub fn compiled_counts(&self) -> (u64, u64) {
        (self.compiled_entries.get(), self.compiled_declines.get())
    }

    /// Answers a backend returned that this boundary refuses — a non-scalar, in
    /// practice. Counted in the declines above as well, because that is what the
    /// machine did with them; separate because a non-zero count here is a backend
    /// bug rather than a fragment limit, and the two should not be read as one
    /// number.
    pub fn compiled_refusals(&self) -> u64 {
        self.compiled_refusals.get()
    }

    /// Every subsequent entry point resets to this stack's fixture rather than
    /// to an empty one. A fixture built once is handed to every test this way.
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
    ///
    /// It cannot go through [`Lowering`]: the cache keys on an address and holds
    /// the program that makes an address an identity, and this expression need
    /// not be in that program. [`Machine::eval_expr_in`] is the one that can.
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
    ///
    /// `e` is borrowed from this machine's program, which is what lets the
    /// clause be lowered once rather than once per case: a property draws
    /// hundreds of points and judges every one of them against this same
    /// expression.
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
        // Before `reset`, so a refusal leaves the previous run's arena alone and
        // the argument's slot is still describable. `reset` restores the
        // fixture's generations, so a slot carried out of an earlier entry point
        // resolves here and reads whatever this run put at that position —
        // silently. See `escape`.
        let boundary = crate::escape::Boundary::EntryPoint { name };
        for arg in &args {
            crate::escape::check(&boundary, arg, span)?;
        }
        self.reset();
        // The same three lines [`Machine::drive`] ends with, and for the same
        // reason: this is an entry point — it resets the world before it starts
        // — so whatever a host handler is holding for it has to be handed back
        // when it ends, on the diagnostic path as much as on the value path.
        // `ply run` reaches `main` through here and through nothing else, so
        // without this a `transaction` left open by `main` is never rolled back
        // and a span left open by `main` is never closed.
        let outcome = self.apply(f, args, span).and_then(|()| self.run());
        // The same close [`Machine::drive`] gives a test. Without it a `simulate`
        // region reached through a named call is neither refused when it is
        // abandoned nor reported to a search, so `Machine::simulated` answers
        // `None` for a run that entered one.
        let outcome = self.close_regions(outcome);
        self.close_open_regions();
        self.end_entry_point();
        outcome
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
                let answered = {
                    let Machine {
                        stack,
                        regions,
                        host_ops,
                        ..
                    } = &mut *self;
                    // A closure rather than a value: a pin is an `Rc`
                    // allocation, and `perform` only calls this for a capture
                    // that can outlive the region it was taken in.
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

    /// A stack that is a value cannot leak from one entry point to the next, so
    /// this restores the world rather than unwinding anything.
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
    ///
    /// The stack goes first, and that is the load-bearing half: a diagnostic
    /// abandons the stack rather than unwinding it, so a `Frame::Resume` left on
    /// it holds a continuation, the continuation holds the pin it took at its
    /// capture, and the close would retain a region nothing can reach any more.
    /// The tree-walker has no stack to abandon and reclaims, so leaving this out
    /// is a `--engine both` divergence on the failure path.
    fn close_open_regions(&mut self) {
        self.stack = Stack::new();
        self.sims.clear();
        self.regions.close_program_regions();
    }

    /// Hands the host runtime every exit path from an entry point.
    ///
    /// Below `close_regions` and above the caller, so it runs on the value path,
    /// the diagnostic path and the spent-budget path alike — which is the whole
    /// of its value, since the exit it exists for is the one where the program
    /// stopped without reaching the operation that would have closed its scope.
    ///
    /// A failure here does not become the entry point's verdict. The program
    /// asked for nothing and did nothing wrong: a connection whose `ROLLBACK`
    /// failed is the run's own resource, and attributing it to whichever test was
    /// running would send a reader looking for a defect in their program. It is
    /// collected as a warning instead, and a caller that reports none is still
    /// correct about the test.
    fn end_entry_point(&mut self) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        if let Err(diagnostic) = runtime.end_entry_point(self.id) {
            self.teardown.push(diagnostic);
        }
    }

    /// What the host runtime reported while closing entry points, and forgotten
    /// here.
    ///
    /// Never failures: see [`Machine::end_entry_point`]. Taken rather than
    /// borrowed because one machine serves many entry points and a caller that
    /// read without clearing would report the third test's discarded connection
    /// again after the fourth.
    pub fn take_teardown_warnings(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.teardown)
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
    /// is checked against the call bound here, so a splice that would make the
    /// machine unbounded is refused at the one place splices land — and against
    /// a frame ceiling too, when one was asked for.
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
            // Built at lowering; this is a refcount bump for a `Str` or a
            // `Bytes` and a copy of an inline variant for everything else.
            NodeKind::Lit(_, value) => self.go_return(value.clone()),

            // The reference-counting pass says whether this is the last read of
            // a binding of this scope. When it is, the value is moved out rather
            // than cloned — the whole of what makes a uniquely-owned value
            // reachable by the operation that could rewrite it. `env` is this
            // arm's to drop, and `take_unique` refuses unless this scope is the
            // only path to the binding, so an emptied binding is unobservable.
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
                let carried = crate::rc::carry(&env, !args.is_empty());
                self.push(
                    Frame::AppCallee {
                        args: args.clone(),
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
                    let carried = crate::rc::carry(&env, fields.len() > 1);
                    self.push(
                        Frame::RecordField {
                            done: Vec::with_capacity(fields.len()),
                            fields: fields.clone(),
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

        // The last thing checked before the handler runs, because the whole
        // point is that the credential has not crossed yet when this fires.
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

        // Beside it, and for the same reason: a handler outlives every region
        // the program opens, so a handle into one has not crossed yet when this
        // fires.
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
                check_host_answer(&operation, declaration.path, &value, span)?;
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
            // Parked until the host answers, which may be after every region
            // open here has closed.
            let k = k.pinned(self.regions.pin());
            let live = region_mut(&mut self.sims, region).expect("the region was just found");
            live.sched.park_on_host(k, pending, span)?;
            return self.schedule();
        }

        // Outside one a `Pending` has nowhere to park, so the machine drives the
        // runtime until the token resolves. This is the one place in the
        // language where a Ply computation blocks a real thread.
        let value = rt.block_on(pending)?;
        check_host_answer(&operation, declaration.path, &value, span)?;
        self.go_return(value);
        Ok(())
    }

    /// Who is performing, as a host handler holding scoped state has to key it.
    ///
    /// The innermost live region's running task, and `None` when control is
    /// inside no region at all. `None` is an identity and not a gap: an entry
    /// point that never spawned is one thread of control from start to finish,
    /// and a scope it opened belongs to it.
    fn performing_task(&self) -> Option<crate::sim::TaskId> {
        let region = self.host_region()?;
        self.sims
            .iter()
            .find(|live| live.id == region)
            .and_then(|live| live.sched.current())
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
    ///
    /// Two tests hold two region stacks, so `ply_test::shared_footprint` may
    /// drop `cell` atoms; two tasks in one simulated run share **one** stack,
    /// and a dependence relation that drops them prunes away every shared-memory
    /// race in the corpus while reporting a *larger* reduction for having done
    /// it.
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
    ///
    /// Allocation has no location to name — that is the point of it — so it is
    /// its own kind of access, dependent with every other allocation. Without it
    /// two tasks that each open a private `with_cell` look like tasks that touch
    /// nothing, and §6.1's soundness condition is false of them: run in the
    /// other order they reach a *different arena*, because the two slots are
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
            // A closure the tree-walker made, handed in through `call`. Its body
            // is a deep clone rather than a node of the program, so it cannot go
            // through `Lowering`; `closure_code` remembers the last one, which
            // is what a closure applied in a loop needs and is all a value the
            // program can mint may be allowed to hold.
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
            // The interpreted path moves these into the callee's `Env`; a scalar
            // carries no refcount and no `Drop`, so dropping them here is the
            // same observation. `argv::give` refuses a non-empty vector, so the
            // buffer must be emptied to stay in the free list.
            args.clear();
            crate::argv::give(args);
            // The bound was charged in `compiled_answer` — a zero budget declines
            // — so what is left is the frame bound `push` checks, in the order
            // the interpreted path below checks them.
            //
            // Returning through `go_return` rather than handing the value back
            // directly is what writes the memo: `ret` pops this frame and
            // `frame::dispatch` performs `remember_constant` on exactly the terms
            // an interpreted call does.
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

    /// The memo is not consulted inside a `simulate` region, and nothing is
    /// written to it from one.
    ///
    /// A pure definition may open its own `with_cell` region, and an allocation
    /// is an [`Access::Alloc`] the search depends on. Skipping one would change
    /// what a schedule records, which is the one thing partial-order reduction
    /// and seeded replay are read off. Outside a region there is no trail and a
    /// cell cannot escape the `with_cell` that made it, so the allocation is
    /// unobservable and the substitution is exact.
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
    ///
    /// `&self` is the load-bearing part: nothing is committed until there is a
    /// value, so a decline restores nothing because nothing was disturbed. The
    /// only mutation is three counters behind [`Cell`], and they are read by
    /// harnesses rather than by the program.
    ///
    /// Every gate a call must clear is in [`crate::compiled::admit`], which
    /// answers with the gate that refused rather than with a bare `None`: a
    /// refusal carrying no reason is a refusal some *other* gate can satisfy,
    /// and one of these was unarmed for exactly that reason. What is left here
    /// is the machine's own half — the backend lookup, which is the whole of the
    /// shipping cost, and the [`crate::compiled::CarriedTypes::answer_crosses`]
    /// test on the answer.
    ///
    /// > **Corrected in place (2026-08-31).** This read *"and the
    /// > [`crate::compiled::crossable`] test on the answer"*, which was the
    /// > whole of it until the answer test began reading the definition's
    /// > declared return type.
    ///
    /// The [`Frame::Call`] at the call site is pushed *after* `enter` returns.
    /// That is sound only because `enter` is handed no route back into this
    /// machine, so nothing can observe the stack while a native body runs and
    /// `note_step_site`, `err_call_limit` and [`Stack::calls`] have no reader to
    /// serve. Give a backend a callback and that push must move above `enter`,
    /// and the bailout stops being free.
    /// The seam's census: which gate would refuse this call, counted whether or
    /// not a backend is attached. See `crate::census`.
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
        // The same predicate by the other route: a walk over the declared types
        // rather than the per-definition `Denotes` the table precomputed. See
        // `census::Counts::carried_sig_walked`.
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
        // The widest rung, attributed: only for calls the shallow twin carries
        // and the deep one does not, so this counts the gap and nothing else.
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
        // The type-level alternative to a deep walk: every gate but the shape
        // one, and the arguments decided from the declared parameter types.
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

        // The SHIPPING type gate, asked with the value-kind test removed. On a
        // program the checker accepted this must equal `admitted`: a value's
        // kind follows its declared type, so the kind test refuses nothing the
        // type test admits. It is counted rather than argued —
        // `the_kind_test_refuses_nothing_the_type_test_admits_over_a_corpus`
        // reads the two numbers off a corpus run.
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
        // A native body pends no frames, so it cannot honour a ceiling counted
        // in them — `enter` is handed the call budget and there is nothing else
        // to hand it. Rather than let an engine-local resource guard decide an
        // answer only one of the three strategies could give, a machine that was
        // asked for one enters nothing. This is the seam's side of item 9.
        //
        // It is deliberately NOT a `Gate` inside `admit`: `admit` takes
        // properties of the CANDIDATE, and a frame ceiling is a property of the
        // MACHINE, like the backend lookup above it.
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
            // The answer test, and it is `admit`'s argument test asked once more
            // at the other end: the declared RETURN type is carried and the
            // answer is of the kind it denotes, or the answer is childless and
            // the old `crossable` rule carries it unchanged. See
            // `CarriedTypes::answer_crosses`, which also records what a
            // container answer stops proving.
            Some(value) if self.carried_types().answer_crosses(name, &value) => {
                self.compiled_entries.set(self.compiled_entries.get() + 1);
                Some(value)
            }
            // Refused in every profile, on purpose. A `debug_assert!` here would
            // make the boundary behave one way under `cargo test` and another in
            // release, and the release half would be the one nobody ran.
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

    /// What a compiled body is handed no route to move. Debug only, and its value
    /// is not today's backend — it is that adding a callback, or handing a
    /// backend an arena, goes red here instead of breaking `note_step_site` in
    /// silence.
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
            let code = stmt.code().clone();
            // The continuation carries only what it still reads. The statement
            // itself evaluates under the full scope, so this is a `drop` placed
            // *before* the operation that could reuse what it frees rather than
            // after it — which is the ordering the whole reuse story rests on.
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
        if q.is_bare() {
            match env.lookup(q.symbol()) {
                Some(Slot::Live(v)) => return Ok(v.clone()),
                // The reference-counting pass called this binding dead and it
                // was read anyway. Falling through would find an outer binding
                // of the same name, or the prelude, and answer with a value
                // nobody wrote — so it stops here, and it is Ply's fault.
                Some(Slot::Released) => return Err(err_released(q, self.current)),
                None => {}
            }
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

    /// The closure for a program-wide name, lowering its body the first time
    /// **any** machine over this program reaches it.
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

    /// The bound both engines share. The stack is a parameter because a splice
    /// is refused before it is adopted, so the calls to name are the ones on the
    /// stack that would have been installed, not the ones on the current one.
    fn err_call_limit(&self, span: Span, stack: &Stack) -> Diagnostic {
        limit::err_recursion_limit(span, NESTED_CALLS, self.max_calls, &innermost_calls(stack))
    }

    /// Only reachable through [`Machine::with_max_frames`], and phrased so a
    /// reader can tell it apart from the bound both engines share: this one says
    /// which engine ran out of what, and never the words "recursion limit".
    fn err_frame_ceiling(&self, span: Span, max: usize, stack: &Stack) -> Diagnostic {
        limit::err_frame_ceiling(span, max, self.max_calls, &innermost_calls(stack))
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

/// A binding the reference-counting pass dropped, read afterwards.
///
/// Ply's fault, not the program's, and loud on purpose: the alternative is a
/// lookup that walks past the released binding, finds an outer one of the same
/// name or a prelude item, and answers with a value the program never wrote.
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

/// Whether a value handed across the boundary holds a credential anywhere.
///
/// A full walk rather than a top-level check, because the argument a handler
/// receives is usually a record or a list and the credential is a field of it —
/// which is exactly the shape a `config`-sourced password reaches an outbound
/// request in. Bounded by the same host-stack growth every other walk over a
/// `Value` uses, and paid only per host operation rather than per call.
/// The host boundary crossed the other way.
///
/// A handler cannot have obtained a handle into the program's memory except by
/// echoing one back, and no shipped registration declares a return type that
/// could hold one — so this refuses a forgery rather than a shape a driver is
/// entitled to produce. Wired at every route an answer takes: inline,
/// [`HostRuntime::block_on`], and the scheduler's poll of a parked token.
///
/// [`HostRuntime::block_on`]: crate::host::HostRuntime::block_on
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

/// `E0439` — a credential reached a host operation whose registration does not
/// declare that it may receive one.
///
/// Ply's fault in the sense `E0427` is: the boundary's own account of itself
/// disagrees with what crossed it. The argument's *position* is named and its
/// value is not, which is the whole discipline of this milestone in one
/// diagnostic.
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

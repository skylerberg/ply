//! A backend a shipping command can attach, and eight ways of being wrong.
//!
//! The seam (`crates/ply-eval/src/compiled.rs`) is the seam. Until this module existed nothing on the
//! shipping side of it implemented [`Compiled`], so `ply test` could not install
//! a backend, the result-cache rule was unenforced *because it was unreachable*,
//! and the eight deliberately wrong backends that police the seam lived in a
//! crate with its own `[workspace]` and its own toolchain. `ADR 0026` §4.1
//! decided that a backend is reachable from a shipping command and §4.5 made
//! *policeability* the condition on one shipping at all:
//!
//! > No backend may ship until `ply test --engine both` can attach one and catch
//! > the eight. Not because policing is more valuable than speed, but because it
//! > is *upstream* of it.
//!
//! This module is that condition discharged, and it is deliberately not a code
//! generator. What it provides:
//!
//! > **A code generator arrived on 2026-08-31 and this module is what polices
//! > it.** `crates/ply-codegen` implements [`Compiled`] and [`Policed`] over
//! > the same seam, and `ply test --backend cranelift` installs it. Nothing
//! > below is withdrawn — this module is still not a code generator and
//! > [`Reference`] is still what runs where there is none — but two shapes here
//! > changed to make a second backend possible, and they are the reason to read
//! > [`Provider`] and [`Policed`] before adding a third:
//! >
//! > * [`Mutant`] wraps an `Rc<dyn Policed>` instead of an `Rc<Reference>`, so
//! >   the eight corruptions apply to any backend rather than to this one.
//! > * The counters moved out of [`Fragment`] into [`Counters`], which a
//! >   [`Provider`] owns and hands to every backend it builds — otherwise a
//! >   second backend would be policed by mutations reporting a different run's
//! >   numbers.
//!
//! - [`Reference`] — a backend whose "compiled code" is a second tree-walker
//!   over its own copy of the program, restricted to a **carried-signature
//!   fragment** — every parameter and the return type in `Int | Bool | Bytes`.
//!   It answers the right value for every call the seam admits and
//!   inside its fragment, which is what makes the *accept* path reachable from a
//!   command a user runs. It is slower than the machine and says so; it exists
//!   to be policed, not to be fast.
//!
//!   > **Half of that is withdrawn on a measurement, 2026-08-30 (ADR 0030).**
//!   > *"It is slower than the machine"* is true of the case ADR 0026 measured —
//!   > a body this backend **declines** is re-run to exhaustion once per offer,
//!   > 26.45 s against 0.04 s over a 20,000-deep ladder — and false of the case
//!   > that decides whether entering is worth anything. Over the Ply front end
//!   > (`spikes/ply-parser` parsing `examples/`, 333,851 bytes) it takes 190,618
//!   > entries covering 296,316 body calls and does them in **0.0800 s against
//!   > the machine's 0.2900 s — 270 ns a body call against 979, 3.63× faster** —
//!   > and the whole run is **1.089×** faster with it attached, counterbalanced
//!   > arms, null control at 0.000%. The mechanism is not that a tree-walker is a
//!   > better engine: run as one over the same program it is 1.51× *slower*, and
//!   > 3.11× slower over the lexer alone. It is that the machine's per-call
//!   > protocol (ADR 0020 §6.3: step, dispatch and refcount, 70.3% of executed
//!   > time) is nearly the whole cost of a body made of scalars and `Bytes`,
//!   > which is exactly what this fragment is.
//!   >
//!   > *"It exists to be policed, not to be fast"* stands, and the second clause
//!   > is now a statement about intent rather than about speed.
//! - [`Mutation`] and [`Mutant`] — the eight configurations of
//!   `crates/ply-codegen-spike/src/wrong.rs`, reproduced over [`Reference`]
//!   rather than over a cranelift fragment, so `cargo test --workspace` and
//!   `ply test` both reach them. ADR 0026 §4.5 item 2 is the argument that this
//!   is possible: *"The mutations do not need a code generator; they need
//!   something that answers."*
//!
//! # What the fragment is, and why it is not "everything"
//!
//! A real backend compiles some definitions and not others, and the calls it is
//! *offered* are a superset of the ones it has a body for — `compiled::admit`
//! gates on the shape of the **arguments** and never on the return type, so a
//! definition taking `Int` and returning a record is offered and must be
//! declined. That gap is where [`Mutation::Unoffered`] lives, and a backend that
//! answered for everything would have no gap to corrupt.
//!
//! So [`Reference`]'s fragment is **the definitions whose whole signature is
//! carried by the seam** — every parameter and the return type `Int`, `Bool` or
//! `Bytes`, which is `compiled::crossable`'s list read off a published scheme
//! rather than off a value. It is a static fact, computed once per run.
//! Everything else is a registry miss and is declined without being run.
//!
//! > **Widened with the seam (2026-08-30).** This read: *"So [`Reference`]'s
//! > fragment is the one ADR 0019 §5 describes and
//! > `crates/ply-codegen-spike/src/measure.rs` registers: **the definitions
//! > whose whole signature is scalar** — every parameter and the return type
//! > `Int` or `Bool`."* Two things are withdrawn. The list is no longer two
//! > kinds, and the fragment is no longer *the one the spike registers*: the
//! > spike's `entry::scalar_signature` is unchanged at `Int | Bool`, so the two
//! > registries have deliberately parted company and this one is the wider.
//! > The spike may not be depended on (ADR 0016 §3.5), which is why its
//! > registry was never the definition of this one and is now visibly not.
//!
//! # The budget is honoured by construction
//!
//! `budget` is the machine's remaining nested calls. [`Reference`] runs the body
//! on an [`Interp`] whose own `max_calls` is set to exactly that, so a body that
//! would outrun the machine's bound raises inside the inner evaluator and is
//! turned into a decline — the machine then re-evaluates and raises its own
//! diagnostic, which is the guarantee `limit.rs` keeps in both engines.
//! [`Mutation::ExceedsBudget`] is the corruption of that one line.
//!
//! # Why a wrong backend is in production source at all
//!
//! Because the rule it exists to check is a rule about a *shipping command*.
//! `CONTRIBUTING.md` §"The one rule" names a green result over unexplored space
//! as this project's most expensive defect class, and `ply test --engine both`
//! reporting `0 failed` with a backend attached is exactly that shape unless
//! something has been seen to make it red. Nothing but a user-runnable wrong
//! backend can do that. The mutations are inert unless a caller names one:
//! nothing constructs a [`Mutant`] on the default path, and
//! [`Fragment::attach`] builds a bare [`Reference`] for [`Mutation::None`].

use crate::compiled::Compiled;
use crate::interp::Interp;
use crate::value::Value;
use ply_core::CheckOutput;
use ply_core::ty::Type;
use ply_span::{Span, Symbol};
use ply_syntax::ast::Program;
use ply_syntax::resolve::Resolved;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

/// One run's backend: the program it answers for, the definitions it has bodies
/// for, and the counters every worker's backend adds to.
///
/// Leaked on construction, and it has to be: [`crate::Machine::set_compiled`]
/// takes a `'static` trait object, for the dropck reason its own field
/// documents, so a backend may not borrow the program the machine is running.
/// One leak per run rather than one per worker is the whole reason this type is
/// separate from [`Reference`] — a `ply test --backend` run builds a backend per
/// worker per group, and each of those would otherwise clone an AST.
///
/// The counters are atomic because the workers are threads and the numbers are
/// the run's rather than a worker's. They are what makes "the corruption fired"
/// checkable *before* "the corruption was caught", which is the middle step of
/// `mutations.rs`'s three and the one usually missing.
pub struct Fragment {
    /// The address of the `Program` the machine is running, for
    /// [`Compiled::describes`]. Never dereferenced — a `usize` rather than a
    /// `*const Program` so this type stays `Sync`.
    origin: usize,
    program: &'static Program,
    resolved: &'static Resolved,
    check: &'static CheckOutput,
    /// The scalar-signature definitions, by program-wide name.
    members: BTreeSet<Symbol>,
    counters: Counters,
}

/// What one run's backend was asked, summed over every worker.
///
/// Split out of [`Fragment`] when a second [`Provider`] appeared, and the split
/// is the point rather than tidiness: [`Mutant`] reads these counters, so a
/// backend that carried its own private set would be policed by mutations that
/// reported a different run's numbers. A [`Provider`] owns one of these and
/// hands it to every backend it builds.
///
/// Atomic because the workers are threads and the numbers are the run's rather
/// than a worker's. They are what makes "the corruption fired" checkable
/// *before* "the corruption was caught", which is the middle step of
/// `crates/ply-cli/tests/backend.rs`'s three and the one usually missing.
#[derive(Default)]
pub struct Counters {
    offered: AtomicU64,
    offered_target: AtomicU64,
    fired: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
}

impl Counters {
    /// One offer, counted, and whether it carried a `Bytes` in.
    ///
    /// Every [`Compiled::enter`] built over a [`Provider`] routes through this
    /// rather than touching `offered` itself, so the two counters cannot drift
    /// apart the day a third backend appears.
    pub fn note_offer(&self, args: &[Value]) {
        self.offered.fetch_add(1, Ordering::Relaxed);
        if args.iter().any(|a| matches!(a, Value::Bytes(_))) {
            self.bytes_in.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// An answered [`Value::Bytes`], counted before any mutation touches it.
    pub fn note_bytes_out(&self) {
        self.bytes_out.fetch_add(1, Ordering::Relaxed);
    }

    /// An offer naming the one definition a targeted mutation corrupts.
    pub fn note_offered_target(&self) {
        self.offered_target.fetch_add(1, Ordering::Relaxed);
    }

    /// An answer a mutation actually changed.
    pub fn note_fired(&self) {
        self.fired.fetch_add(1, Ordering::Relaxed);
    }

    pub fn offers(&self) -> Offers {
        Offers {
            offered: self.offered.load(Ordering::Relaxed),
            offered_target: self.offered_target.load(Ordering::Relaxed),
            fired: self.fired.load(Ordering::Relaxed),
            bytes_in: self.bytes_in.load(Ordering::Relaxed),
            bytes_out: self.bytes_out.load(Ordering::Relaxed),
        }
    }
}

/// What a run's backend was asked and what it did with it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Offers {
    /// Calls the machine offered this backend, over every worker.
    pub offered: u64,
    /// Offers naming the one definition a targeted mutation corrupts. Zero is a
    /// claim about a gate, which is why it is counted apart.
    pub offered_target: u64,
    /// Calls whose answer a mutation actually changed. Zero means a run said
    /// nothing about the corpus that did not catch it.
    pub fired: u64,
    /// Offers carrying at least one [`Value::Bytes`] argument.
    ///
    /// The widening of `compiled::crossable` is inert unless this is non-zero on
    /// real source, and a corpus run that is green over a seam nothing new
    /// reached is the vacuous pass `CONTRIBUTING.md` §"The one rule" names. So
    /// it is counted rather than assumed, and
    /// `differential_corpus.rs`'s honest-backend test asserts it.
    pub bytes_in: u64,
    /// Entered calls that answered a [`Value::Bytes`], counted before any
    /// mutation touches the answer. The other direction of the same claim: a
    /// `Bytes` argument that crosses in and never comes back out would mean the
    /// widening bought arguments and not returns.
    pub bytes_out: u64,
}

impl Fragment {
    /// The scalar-signature fragment of `program`, over a copy of it.
    ///
    /// The copy is what the answers are computed on, so a backend's evaluation
    /// shares no state at all with the machine's — no arena, no handler stack,
    /// no memo. `describes` still compares against the *original*, so a
    /// bisection that builds a program whose definitions carry the names of the
    /// ones they replace gets a backend that declines to describe it, exactly as
    /// `Machine::set_lowering` does.
    pub fn over(program: &Program, resolved: &Resolved, check: &CheckOutput) -> &'static Fragment {
        let origin = std::ptr::from_ref(program) as usize;
        let copy: &'static Program = Box::leak(Box::new(program.clone()));
        let resolved: &'static Resolved = Box::leak(Box::new(resolved.clone()));
        let check: &'static CheckOutput = Box::leak(Box::new(check.clone()));
        Fragment::build(origin, copy, resolved, check)
    }

    /// The same fragment over a program that is already `'static`, with no copy
    /// at all.
    ///
    /// [`Fragment::over`] clones because a backend may not borrow — see the
    /// `compiled` field on [`crate::Machine`] — and a caller holding a leaked
    /// program has already paid that price. It is what a harness that sweeps one
    /// corpus with eight backends wants: eight fragments, one program, and the
    /// counters fresh each time.
    ///
    /// `origin` is the program the *machine* is running, which is this one, so
    /// [`Compiled::describes`] is true by identity rather than by a comparison
    /// that could be wrong.
    pub fn over_static(
        program: &'static Program,
        resolved: &'static Resolved,
        check: &'static CheckOutput,
    ) -> &'static Fragment {
        Fragment::build(
            std::ptr::from_ref(program) as usize,
            program,
            resolved,
            check,
        )
    }

    fn build(
        origin: usize,
        program: &'static Program,
        resolved: &'static Resolved,
        check: &'static CheckOutput,
    ) -> &'static Fragment {
        let members = check
            .defs
            .iter()
            .filter(|(_, def)| carried_signature(&def.scheme.ty))
            .map(|(name, _)| name.clone())
            .collect();
        Box::leak(Box::new(Fragment {
            origin,
            program,
            resolved,
            check,
            members,
            counters: Counters::default(),
        }))
    }

    /// How many definitions this backend has a body for.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    pub fn holds(&self, name: &Symbol) -> bool {
        self.members.contains(name)
    }

    pub fn offers(&self) -> Offers {
        self.counters.offers()
    }

    /// A backend for one worker's machine, built on that worker's own thread.
    pub fn attach(&'static self, spec: &Spec) -> Rc<dyn Compiled> {
        wrap(Rc::new(Reference::new(self)), spec)
    }
}

/// A run's source of backends: one per run, shared by every worker, and the
/// only route a shipping command has to install one.
///
/// **This trait is what ADR 0026 §4.5's 2026-08-30 note said had to exist**, and
/// the note is worth quoting because the gap it names is exactly what the trait
/// closes:
///
/// > `ply test`'s only install route is `InterpExecutor::with_backend`, whose
/// > signature is `(&'static ply_eval::Fragment, BackendSpec)` — a `Fragment`,
/// > not a `dyn Compiled` … So **the eight corruptions police one implementation
/// > of `Compiled` and there is no route by which a second one is offered to
/// > them.**
///
/// `Send + Sync` because a `ply test` run installs one backend per worker
/// thread and every one of them adds to the same [`Counters`]. `'static`
/// because [`crate::Machine::set_compiled`] takes a `'static` trait object, for
/// the dropck reason its own field documents.
///
/// [`attach`](Provider::attach) is called **on the worker's own thread**: a
/// backend is `Rc` all the way down, and a code generator's compile is work a
/// worker does for itself rather than work a run does once and shares.
pub trait Provider: Send + Sync {
    /// This provider's backend for one worker, corrupted as `spec` asks.
    fn attach(&'static self, spec: &Spec) -> Rc<dyn Compiled>;

    /// What `--backend` calls this, for a report a user reads.
    fn name(&self) -> &'static str;

    /// How many definitions this provider has a body for.
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// What every worker's backend was asked, summed.
    fn offers(&self) -> Offers;

    /// What this provider spent compiling, or `None` for one that compiles
    /// nothing.
    ///
    /// **`None` rather than a zeroed [`Compilation`], and the difference is not
    /// cosmetic.** [`Fragment`] builds an `Interp` per worker and could count
    /// them, but it could not time them: this crate may not read the host's
    /// clock at all — `crates/ply-eval/tests/simulated_handlers.rs`'s
    /// `the_evaluator_reads_no_host_clock_and_no_host_entropy` bans the type by
    /// name from this source, and it has caught a measurement before (ADR 0030
    /// §4). A zero here would read as "the tree-walker takes no time to build",
    /// which is a claim nothing in this crate is allowed to make.
    fn compilation(&self) -> Option<Compilation> {
        None
    }

    /// Workers this provider could not build a backend for.
    ///
    /// Zero for a provider that cannot fail — [`Fragment`] builds an
    /// [`Interp`], which is infallible — and the default reflects that. A code
    /// generator can fail on a worker, and a worker with no bodies declines
    /// every call, so a run that reported green over one would be green over a
    /// seam nothing reached. `ply test` turns a non-zero count into an
    /// `INTERNAL_ERROR` and fails.
    fn unbuilt(&self) -> u64 {
        0
    }
}

impl Provider for Fragment {
    fn attach(&'static self, spec: &Spec) -> Rc<dyn Compiled> {
        Fragment::attach(self, spec)
    }

    fn name(&self) -> &'static str {
        "reference"
    }

    fn len(&self) -> usize {
        Fragment::len(self)
    }

    fn offers(&self) -> Offers {
        Fragment::offers(self)
    }
}

/// What a run spent turning a program into compiled code.
///
/// Split into the two halves because they scale differently and a single total
/// hides which one a reader is looking at: the analysis is whole-program and is
/// paid **once**, and the code generation is paid **per worker**, because a
/// compiled unit owns a constant pool of [`Value`]s and a `Value` is `Rc` all
/// the way down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Compilation {
    /// Nanoseconds deciding *what* to compile. Whole-program, paid once per
    /// run.
    pub analysis_nanos: u64,
    /// Nanoseconds inside the code generator, summed over every backend built.
    pub codegen_nanos: u64,
    /// Backends built.
    ///
    /// One per worker that was asked for one, **plus one** for a provider that
    /// compiles a unit up front to prove it can — which is why a `-j 1` run
    /// reports two. `ply_codegen::Cranelift::over` does exactly that, so that
    /// "this host has no cranelift backend" is a diagnostic before the run
    /// rather than a worker that silently declines every call.
    pub units: u64,
}

/// A backend the eight corruptions can wrap.
///
/// [`Compiled`] is `describes` and `enter` and nothing else, which is not enough
/// to be policed: two of the eight need operations that are not on it.
/// [`Mutation::Unoffered`] asks the **registry** whether a body exists at all,
/// which is the difference between a decline and a name that was never compiled
/// — and that difference is the gap the mutation lives in.
/// [`Mutation::ExceedsBudget`] re-runs the body on fuel that is deliberately
/// **not** the machine's budget. So a backend that only implements [`Compiled`]
/// can be installed and cannot be corrupted, which is a backend nothing has
/// checked.
///
/// The three methods here are exactly those two plus the honest answer, and
/// `enter` is deliberately **not** one of them: [`Compiled::enter`] counts an
/// offer, and a mutant that called it would count every offer twice.
pub trait Policed: Compiled {
    /// The run's counters, which every backend over one provider shares.
    fn counters(&self) -> &'static Counters;

    /// Whether this backend has a body for `name` at all.
    fn holds(&self, name: &Symbol) -> bool;

    /// The honest answer for a call, **without** counting an offer.
    fn answer(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value>;

    /// The body on an arbitrary bound, whatever the machine allowed. The only
    /// caller is [`Mutation::ExceedsBudget`], which is the corruption of the
    /// bound.
    fn run_with_fuel(&self, name: &Symbol, args: &[Value], fuel: usize) -> Option<Value>;
}

/// One worker's backend, corrupted as `spec` asks — the one place a [`Mutant`]
/// is built, so that no provider can install a wrapper the others do not.
///
/// [`Mutation::None`] with no target is the honest backend and is *not*
/// wrapped: a wrapper that is inert is still a wrapper, and the control every
/// other result is read against has to be the unwrapped thing.
pub fn wrap(inner: Rc<dyn Policed>, spec: &Spec) -> Rc<dyn Compiled> {
    match (&spec.mutation, &spec.target) {
        (Mutation::None, None) => inner,
        _ => Rc::new(Mutant {
            inner,
            mutation: spec.mutation.clone(),
            target: spec.target.clone(),
            previous: RefCell::new(None),
        }),
    }
}

/// Every parameter and the return type `Int`, `Bool` or `Bytes`, which is the
/// whole of what `compiled::crossable` carries.
///
/// Read off the published scheme rather than off the body: a fragment is a
/// static fact a registry is built from, and a backend that decided per call
/// whether it had a body would be deciding it from the arguments the machine
/// already gated on.
///
/// > **Renamed and widened (2026-08-30).** This was `scalar_signature` over
/// > `Int | Bool`, and the name became a lie the moment `compiled::crossable`
/// > grew a third kind that is not a scalar. What it has always meant is "every
/// > position in this signature is a kind the seam carries", so it is now named
/// > that and reads its list from the same place the seam does.
/// >
/// > The widening is what turns a class of *offered and declined* call into an
/// > entered one, and that class was the majority: over `examples/` and
/// > `tests/fixtures/` before this change, 122,853 calls cleared every gate and
/// > 19,009 had a signature this predicate accepted — **84.5% of admitted calls
/// > were offered and then declined on the return type**, `std.router.hex_char`
/// > (`Int -> Bytes`, 65,560 calls) and `std.router.escaped` (32,780) being most
/// > of it. `compiled::admit` gates arguments and never the return, which is the
/// > gap [`Mutation::Unoffered`] lives in; widening this narrows that gap
/// > without closing it, and `an_answer_for_a_definition_with_no_body_is_caught_over_the_corpus`
/// > is what fails if it ever does close.
pub(crate) fn carried_signature(ty: &Type) -> bool {
    let Type::Fn { params, ret, .. } = ty else {
        return false;
    };
    params.iter().chain([ret.as_ref()]).all(is_carried)
}

/// The declared types whose values `compiled::crossable` carries.
///
/// `Bytes` is the third, and it is a nominal type with no arguments exactly as
/// `Int` and `Bool` are, so nothing about the shape of this test changes with
/// it. A `Type::Var` is refused for the reason it is refused everywhere else
/// here: an unresolved variable can be instantiated at a closure.
fn is_carried(ty: &Type) -> bool {
    match ty {
        Type::Con(name, args) => {
            args.is_empty() && matches!(name.as_str(), "Int" | "Bool" | "Bytes")
        }
        _ => false,
    }
}

/// A backend whose compiled code is a second tree-walker.
///
/// It is not a JIT and does not pretend to be one. What it is, is a backend that
/// answers the right value for every call this boundary admits *and* that has a
/// fragment it can miss — which makes the accept path, the decline path and the
/// registry-miss path all reachable from `ply test`.
pub struct Reference {
    fragment: &'static Fragment,
    inner: RefCell<Interp<'static>>,
}

impl Reference {
    fn new(fragment: &'static Fragment) -> Reference {
        Reference {
            fragment,
            inner: RefCell::new(Interp::new(
                fragment.program,
                fragment.resolved,
                fragment.check,
            )),
        }
    }

    /// The body with an arbitrary bound, whatever the registry says. The only
    /// caller outside [`Policed::answer`] is [`Mutation::ExceedsBudget`],
    /// which is the corruption of the bound, and [`Mutation::Unoffered`], which
    /// is the corruption of the registry.
    fn run(&self, name: &Symbol, args: &[Value], fuel: usize) -> Option<Value> {
        let mut inner = self.inner.try_borrow_mut().ok()?;
        inner.set_max_calls(fuel);
        match inner.call(name.as_str(), args.to_vec(), Span::DUMMY) {
            Ok(value @ Value::Bytes(_)) => {
                self.fragment.counters.note_bytes_out();
                Some(value)
            }
            Ok(value @ (Value::Int(_) | Value::Bool(_))) => Some(value),
            // A registry hit whose body raised, or answered something this
            // boundary does not carry. Declining is the contract: the machine
            // re-evaluates and raises its own diagnostic, which is what makes
            // the *interpreter's* code, spans and notes the ones a user sees.
            _ => None,
        }
    }
}

impl Policed for Reference {
    fn counters(&self) -> &'static Counters {
        &self.fragment.counters
    }

    fn holds(&self, name: &Symbol) -> bool {
        self.fragment.holds(name)
    }

    /// The honest answer: the body, run under exactly the machine's remaining
    /// call budget, or `None` for a registry miss, a non-scalar answer, or a
    /// body that raised — including the body that raised *because* it outran the
    /// budget, which is the decline the machine's own bound depends on.
    fn answer(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
        if !self.fragment.holds(name) {
            return None;
        }
        self.run(name, args, budget)
    }

    fn run_with_fuel(&self, name: &Symbol, args: &[Value], fuel: usize) -> Option<Value> {
        self.run(name, args, fuel)
    }
}

impl Compiled for Reference {
    fn describes(&self, program: &Program) -> bool {
        self.fragment.origin == std::ptr::from_ref(program) as usize
    }

    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
        self.fragment.counters.note_offer(args);
        let answer = Policed::answer(self, name, args, budget);
        // Measurement scaffolding, off unless `PLY_SEAM_CENSUS` is set. It
        // records the name of a call that was *entered* and not of one that was
        // merely offered: a registry miss is a hash lookup and no evaluation, so
        // `answer.is_some()` is load-bearing, and dropping it reports the
        // offered set under the entered set's name — deleted and re-run, it
        // turns `32 distinct, 188792 entries` into `33 distinct, 188805` and
        // lists `lexer.lex`, which is declined every time it is offered
        // (ADR 0030 §7).
        //
        // It is a count and it may not become a duration. This crate may not
        // read the host's clock at all — `simulated_handlers.rs`'s
        // `the_evaluator_reads_no_host_clock_and_no_host_entropy` bans the type
        // by name from this source, and it caught a first version of exactly
        // this measurement (ADR 0030 §4). What a backend *costs* is measured
        // from outside the process.
        if crate::census::enabled() && answer.is_some() {
            let label = name.as_str().to_string();
            crate::census::with(|c| *c.entered_names.entry(label).or_default() += 1);
        }
        answer
    }
}

/// One way of being wrong.
///
/// Transcribed from `crates/ply-codegen-spike/src/wrong.rs`, whose table of what
/// each attacks is reproduced here because the claim a mutation makes is the
/// only thing that says whether catching it is worth anything:
///
/// | mutation | the claim it attacks |
/// | --- | --- |
/// | [`Mutation::OffByOne`] | a compiled arithmetic result is checked, not assumed |
/// | [`Mutation::Inverted`] | so is a compiled comparison |
/// | [`Mutation::Stale`] | an answer is tied to *this* call's arguments |
/// | [`Mutation::WrongType`] | the seam marshals a kind as well as a value — over all three kinds it carries, `Bytes` included |
/// | [`Mutation::Unoffered`] | a backend may not answer for a body it does not have |
/// | [`Mutation::ExceedsBudget`] | `budget` is the machine's bound and not a hint |
/// | [`Mutation::Answers`] | a call the machine must never offer at all |
///
/// The last one is not a backend defect and cannot be demonstrated by a backend
/// alone: `compiled::admit` refuses any definition whose published
/// effect row is non-empty or that performs under a `handle` of its own, so a
/// backend is never asked. What a [`Mutant`] can do is stand ready to answer one
/// and count how often it was asked — zero, while those gates hold.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Mutation {
    /// The honest answer, so a harness can check that the wrapper itself changes
    /// nothing. Every mutation result is read against this.
    #[default]
    None,
    /// `Int(n)` becomes `Int(n + 1)`: the arithmetic is off by one.
    OffByOne,
    /// `Bool(b)` becomes `Bool(!b)`: the comparison is inverted.
    Inverted,
    /// This call answers what the *previous* entered call answered.
    Stale,
    /// The same information in the wrong kind — `Bool` where the definition
    /// returns `Int`, `Int` where it returns `Bool`, and the length where it
    /// returns `Bytes`. All three are crossable, so the seam carries them and
    /// whatever notices has to be downstream of it.
    WrongType,
    /// Answers for a name this backend has no body for, instead of declining.
    /// The invented answer is the first `Int` argument, which is the shape a
    /// plausible bug takes — a registry that answered for the wrong entry point
    /// would look like this rather than like `Int(0)`.
    Unoffered,
    /// Runs the body with more fuel than the machine allowed instead of
    /// declining. `None` is unlimited; `Some(k)` is `k` times the budget. The
    /// unbounded form is a native runaway with no bound at all and takes the
    /// process down with it, which is not a wrong answer and cannot be reported
    /// from inside the process it kills.
    ExceedsBudget(Option<u32>),
    /// Answers this value for the target name whatever the machine asked, and
    /// whether or not a body exists. The only mutation that can accept a call
    /// the machine must decline — and it only ever gets the chance if a gate in
    /// `compiled::admit` is removed.
    Answers(i64),
}

impl Mutation {
    pub fn describe(&self) -> String {
        match self {
            Mutation::None => "nothing".to_string(),
            Mutation::OffByOne => "every `Int` answer is one too high".to_string(),
            Mutation::Inverted => "every `Bool` answer is inverted".to_string(),
            Mutation::Stale => "every answer is the previous call's".to_string(),
            Mutation::WrongType => {
                "`Int` answers come back as `Bool` and back, and `Bytes` as its length".to_string()
            }
            Mutation::Unoffered => {
                "names with no compiled body are answered rather than declined".to_string()
            }
            Mutation::ExceedsBudget(None) => {
                "the machine's call budget is ignored entirely".to_string()
            }
            Mutation::ExceedsBudget(Some(k)) => {
                format!("bodies run with {k}x the budget the machine allowed")
            }
            Mutation::Answers(v) => format!("the target is answered `{v}` unconditionally"),
        }
    }
}

/// Which of the backends a command can install.
///
/// This enum names a **choice a command makes**, not an implementation this
/// crate holds: `ply-eval` implements [`Reference`] and depends on no code
/// generator. [`Kind::Cranelift`] is served by `crates/ply-codegen`, which
/// implements [`Provider`] over the same seam, and it is named here because the
/// `--backend` grammar is one grammar and splitting it across two crates would
/// give a user two half-answers to `--backend nonsense`.
///
/// A `ply-cli` built without a code generator refuses [`Kind::Cranelift`] with a
/// diagnostic that says so; it does not silently install the tree-walker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    /// [`Reference`]: a second tree-walker over the carried-signature fragment.
    #[default]
    Reference,
    /// `ply_codegen::Cranelift`: native code, compiled at install time.
    Cranelift,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Reference => "reference",
            Kind::Cranelift => "cranelift",
        }
    }
}

/// Which backend a command was asked for, and which definition it corrupts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Spec {
    /// Which backend answers. The mutations wrap whichever it is — see
    /// [`Policed`].
    pub kind: Kind,
    pub mutation: Mutation,
    /// The one definition to corrupt, or every one of them. A whole-corpus
    /// mutation says whether the corpus notices *at all*; a targeted one says
    /// whether it notices *that function*, which is the coverage question.
    pub target: Option<Symbol>,
}

impl Spec {
    pub fn honest() -> Spec {
        Spec::default()
    }

    pub fn describe(&self) -> String {
        match &self.target {
            Some(name) => format!("{} (only `{name}`)", self.mutation.describe()),
            None => self.mutation.describe(),
        }
    }
}

/// Parses a `--backend` argument.
///
/// `reference` and `cranelift` are the two honest backends.
/// `wrong:<mutation>[@<function>]` is one of the eight, plus `=<int>` for the
/// two that carry a number. Named rather than positional so a run's provenance
/// says what was broken: `wrong:off-by-one@store.total` in a log is a
/// reproducible experiment.
///
/// A corruption may name the backend it wraps —
/// `cranelift:wrong:off-by-one@store.total` — and a bare `wrong:` keeps meaning
/// `reference:wrong:`, which is what it meant before a second backend existed.
/// That default is a compatibility choice and it is the conservative one: every
/// `--backend wrong:` invocation recorded in this repository was taken against
/// the tree-walker, and silently re-pointing them at a code generator would
/// change what a re-run of a logged experiment measures.
pub fn parse(spec: &str) -> Result<Spec, String> {
    // A bare backend name, honest. Handled before the prefix split so that
    // `reference:cranelift` and `cranelift:reference` — which are spellings of
    // nothing — fall through to the refusal below instead of quietly installing
    // one half of themselves.
    match spec {
        "reference" => return Ok(Spec::honest()),
        "cranelift" => {
            return Ok(Spec {
                kind: Kind::Cranelift,
                ..Spec::honest()
            });
        }
        _ => {}
    }
    let (backend, rest) = match spec.split_once(':') {
        Some(("cranelift", rest)) => (Kind::Cranelift, rest),
        Some(("reference", rest)) => (Kind::Reference, rest),
        _ => (Kind::Reference, spec),
    };
    let Some(rest) = rest.strip_prefix("wrong:") else {
        return Err(format!(
            "unknown backend `{spec}`; one of `reference`, `cranelift`, or \
             `[<backend>:]wrong:<mutation>` where <mutation> is off-by-one, inverted, stale, \
             wrong-type, unoffered, exceeds-budget[={{k}}] or answers={{int}}, each optionally \
             @<definition>"
        ));
    };
    let (head, target) = match rest.split_once('@') {
        Some((head, target)) => (head, Some(Symbol::new(target))),
        None => (rest, None),
    };
    let (kind, argument) = match head.split_once('=') {
        Some((kind, argument)) => (kind, Some(argument)),
        None => (head, None),
    };
    let mutation = match (kind, argument) {
        ("off-by-one", None) => Mutation::OffByOne,
        ("inverted", None) => Mutation::Inverted,
        ("stale", None) => Mutation::Stale,
        ("wrong-type", None) => Mutation::WrongType,
        ("unoffered", None) => Mutation::Unoffered,
        ("exceeds-budget", None) => Mutation::ExceedsBudget(None),
        ("exceeds-budget", Some(k)) => Mutation::ExceedsBudget(Some(
            k.parse()
                .map_err(|_| format!("`exceeds-budget={k}` needs a whole number"))?,
        )),
        ("answers", Some(v)) => Mutation::Answers(
            v.parse()
                .map_err(|_| format!("`answers={v}` needs a whole number"))?,
        ),
        _ => {
            return Err(format!(
                "unknown mutation `{head}`; one of off-by-one, inverted, stale, wrong-type, \
                 unoffered, exceeds-budget[={{k}}], answers={{int}}, each optionally @<definition>"
            ));
        }
    };
    if matches!(mutation, Mutation::Answers(_)) && target.is_none() {
        return Err(
            "`wrong:answers=` needs a target: `wrong:answers=302@orders.measured`".to_string(),
        );
    }
    Ok(Spec {
        kind: backend,
        mutation,
        target,
    })
}

/// A backend that is wrong on purpose, wrapped around one that is not.
///
/// # What these eight police
///
/// **They police any [`Policed`] backend, which is what changed on 2026-08-31.**
/// This block read, until then:
///
/// > **They police [`Reference`]. They do not police [`Compiled`].** The field
/// > below is the whole argument: `inner` is a concrete `Rc<Reference>`, not an
/// > `Rc<dyn Compiled>`, and it is that way because two of the eight need
/// > operations the trait does not have. [`Mutation::Unoffered`] asks the
/// > *registry* — `fragment.holds(name)` — whether a body exists at all, which
/// > is the difference between a decline and a name that was never compiled;
/// > that difference is the gap the mutation lives in.
/// > [`Mutation::ExceedsBudget`] re-runs the body on fuel that is deliberately
/// > **not** the machine's budget, through `Reference::run`, which is private to
/// > this module. [`Compiled`] is `describes` and `enter` (`compiled.rs`) and
/// > nothing else, so neither operation can be asked of an arbitrary backend.
/// >
/// > The consequence is worth stating where the next backend's author will meet
/// > it, because the shipping path hides it. `ply test`'s one install route is
/// > `ply_test::InterpExecutor::with_backend`, and its parameter is a
/// > `&'static Fragment` rather than a `dyn Compiled` — so the only backends a
/// > user can attach are the ones [`Fragment::attach`] builds, which is
/// > [`Reference`] and these wrappers over it. **A second implementation of
/// > [`Compiled`] — a code generator, say — cannot currently be handed to a
/// > shipping command at all, and if it could, none of the eight would be
/// > wrapping it.**
///
/// The diagnosis was right and the fix is the one it named: the two operations
/// it lists are now [`Policed::holds`] and [`Policed::run_with_fuel`], `inner`
/// is an `Rc<dyn Policed>`, and `with_backend` takes a [`Provider`] instead of a
/// concrete type. `ply_codegen::Cranelift` is the second implementation, and it
/// is wrapped by these eight through the same `wrap` every provider calls.
///
/// **What that does not do is make one backend's result another's.** ADR 0026
/// §4.5 is a condition on *any* backend, and it is discharged per backend: the
/// count of how many of the eight are caught is a property of the backend under
/// them, because what a corruption can bite depends on which definitions the
/// backend has a body for. `crates/ply-cli/tests/backend.rs` runs the whole
/// table against each installed backend rather than against one, for exactly
/// that reason.
pub struct Mutant {
    inner: Rc<dyn Policed>,
    mutation: Mutation,
    target: Option<Symbol>,
    previous: RefCell<Option<Value>>,
}

impl Mutant {
    fn fire(&self, value: Value) -> Option<Value> {
        self.inner.counters().note_fired();
        Some(value)
    }
}

impl Compiled for Mutant {
    fn describes(&self, program: &Program) -> bool {
        self.inner.describes(program)
    }

    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
        let counters = self.inner.counters();
        counters.note_offer(args);
        if self.target.as_ref().is_some_and(|t| t != name) {
            return self.inner.answer(name, args, budget);
        }
        counters.note_offered_target();

        // The two that answer without an honest answer to corrupt.
        match &self.mutation {
            Mutation::Answers(value) => return self.fire(Value::Int(*value)),
            Mutation::Unoffered if !self.inner.holds(name) => {
                let invented = args
                    .iter()
                    .find(|v| matches!(v, Value::Int(_)))
                    .cloned()
                    .unwrap_or(Value::Int(0));
                return self.fire(invented);
            }
            _ => {}
        }

        let honest = self.inner.answer(name, args, budget);
        match (&self.mutation, honest) {
            (Mutation::None | Mutation::Unoffered, answer) => answer,
            (Mutation::OffByOne, Some(Value::Int(n))) => self.fire(Value::Int(n.wrapping_add(1))),
            (Mutation::Inverted, Some(Value::Bool(b))) => self.fire(Value::Bool(!b)),
            (Mutation::WrongType, Some(Value::Int(n))) => self.fire(Value::Bool(n != 0)),
            (Mutation::WrongType, Some(Value::Bool(b))) => self.fire(Value::Int(i64::from(b))),
            // The third kind the seam carries, and the arm that keeps this
            // mutation's claim true of it. `Int` is crossable, so the machine
            // accepts this answer and hands it to the program: whatever notices
            // is downstream of the boundary, which is the whole point.
            (Mutation::WrongType, Some(Value::Bytes(ref b))) => {
                self.fire(Value::Int(b.len() as i64))
            }
            (Mutation::Stale, Some(value)) => {
                let stale = self.previous.borrow_mut().replace(value.clone());
                match stale {
                    Some(stale) if stale.render() != value.render() => self.fire(stale),
                    // The first entry, or a repeat of the last answer: nothing
                    // to be stale about, so this call is honest and is not
                    // counted.
                    _ => Some(value),
                }
            }
            // A body that fits its budget answers the same either way; the
            // mutation is only visible where the honest backend declined.
            (Mutation::ExceedsBudget(times), None) if self.inner.holds(name) => {
                let fuel = match times {
                    None => usize::MAX,
                    Some(k) => budget.saturating_mul(*k as usize),
                };
                match self.inner.run_with_fuel(name, args, fuel) {
                    Some(value) => self.fire(value),
                    None => None,
                }
            }
            (_, answer) => answer,
        }
    }
}

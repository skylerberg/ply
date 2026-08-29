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
//! - [`Reference`] — a backend whose "compiled code" is a second tree-walker
//!   over its own copy of the program, restricted to a **scalar-signature
//!   fragment**. It answers the right value for every call the seam admits and
//!   inside its fragment, which is what makes the *accept* path reachable from a
//!   command a user runs. It is slower than the machine and says so; it exists
//!   to be policed, not to be fast.
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
//! So [`Reference`]'s fragment is the one ADR 0019 §5 describes and
//! `crates/ply-codegen-spike/src/measure.rs` registers: **the definitions whose
//! whole signature is scalar** — every parameter and the return type `Int` or
//! `Bool`. It is a static fact about a published scheme, computed once per run.
//! Everything else is a registry miss and is declined without being run.
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
    offered: AtomicU64,
    offered_target: AtomicU64,
    fired: AtomicU64,
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
            .filter(|(_, def)| scalar_signature(&def.scheme.ty))
            .map(|(name, _)| name.clone())
            .collect();
        Box::leak(Box::new(Fragment {
            origin,
            program,
            resolved,
            check,
            members,
            offered: AtomicU64::new(0),
            offered_target: AtomicU64::new(0),
            fired: AtomicU64::new(0),
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
        Offers {
            offered: self.offered.load(Ordering::Relaxed),
            offered_target: self.offered_target.load(Ordering::Relaxed),
            fired: self.fired.load(Ordering::Relaxed),
        }
    }

    /// A backend for one worker's machine, built on that worker's own thread.
    ///
    /// [`Mutation::None`] with no target is the honest backend and is *not*
    /// wrapped: a wrapper that is inert is still a wrapper, and the control this
    /// module's tests read every other result against has to be the unwrapped
    /// thing.
    pub fn attach(&'static self, spec: &Spec) -> Rc<dyn Compiled> {
        let reference = Rc::new(Reference::new(self));
        match (&spec.mutation, &spec.target) {
            (Mutation::None, None) => reference,
            _ => Rc::new(Mutant {
                inner: reference,
                mutation: spec.mutation.clone(),
                target: spec.target.clone(),
                previous: RefCell::new(None),
            }),
        }
    }
}

/// Every parameter and the return type `Int` or `Bool`, which is the whole of
/// what `compiled::crossable` carries.
///
/// Read off the published scheme rather than off the body: a fragment is a
/// static fact a registry is built from, and a backend that decided per call
/// whether it had a body would be deciding it from the arguments the machine
/// already gated on.
fn scalar_signature(ty: &Type) -> bool {
    let Type::Fn { params, ret, .. } = ty else {
        return false;
    };
    params.iter().chain([ret.as_ref()]).all(is_scalar)
}

fn is_scalar(ty: &Type) -> bool {
    match ty {
        Type::Con(name, args) => {
            args.is_empty() && (name.as_str() == "Int" || name.as_str() == "Bool")
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

    /// The honest answer: the body, run under exactly the machine's remaining
    /// call budget, or `None` for a registry miss, a non-scalar answer, or a
    /// body that raised — including the body that raised *because* it outran the
    /// budget, which is the decline the machine's own bound depends on.
    fn answer(&self, name: &Symbol, args: &[Value], fuel: usize) -> Option<Value> {
        if !self.fragment.holds(name) {
            return None;
        }
        self.run(name, args, fuel)
    }

    /// The body with an arbitrary bound, whatever the registry says. The only
    /// caller outside [`Reference::answer`] is [`Mutation::ExceedsBudget`],
    /// which is the corruption of the bound, and [`Mutation::Unoffered`], which
    /// is the corruption of the registry.
    fn run(&self, name: &Symbol, args: &[Value], fuel: usize) -> Option<Value> {
        let mut inner = self.inner.try_borrow_mut().ok()?;
        inner.set_max_calls(fuel);
        match inner.call(name.as_str(), args.to_vec(), Span::DUMMY) {
            Ok(value @ (Value::Int(_) | Value::Bool(_))) => Some(value),
            // A registry hit whose body raised, or answered something this
            // boundary does not carry. Declining is the contract: the machine
            // re-evaluates and raises its own diagnostic, which is what makes
            // the *interpreter's* code, spans and notes the ones a user sees.
            _ => None,
        }
    }
}

impl Compiled for Reference {
    fn describes(&self, program: &Program) -> bool {
        self.fragment.origin == std::ptr::from_ref(program) as usize
    }

    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
        self.fragment.offered.fetch_add(1, Ordering::Relaxed);
        self.answer(name, args, budget)
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
/// | [`Mutation::WrongType`] | the seam marshals a kind as well as a value |
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
    /// returns `Int`, and `Int` where it returns `Bool`. Both are crossable, so
    /// the seam carries them and whatever notices has to be downstream of it.
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
            Mutation::WrongType => "`Int` answers come back as `Bool` and back".to_string(),
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

/// Which backend a command was asked for, and which definition it corrupts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Spec {
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
/// `reference` is the honest backend. `wrong:<mutation>[@<function>]` is one of
/// the eight, plus `=<int>` for the two that carry a number. Named rather than
/// positional so a run's provenance says what was broken:
/// `wrong:off-by-one@store.total` in a log is a reproducible experiment.
pub fn parse(spec: &str) -> Result<Spec, String> {
    if spec == "reference" {
        return Ok(Spec::honest());
    }
    let Some(rest) = spec.strip_prefix("wrong:") else {
        return Err(format!(
            "unknown backend `{spec}`; one of `reference`, or `wrong:<mutation>` where \
             <mutation> is off-by-one, inverted, stale, wrong-type, unoffered, \
             exceeds-budget[={{k}}] or answers={{int}}, each optionally @<definition>"
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
    Ok(Spec { mutation, target })
}

/// A backend that is wrong on purpose, wrapped around one that is not.
pub struct Mutant {
    inner: Rc<Reference>,
    mutation: Mutation,
    target: Option<Symbol>,
    previous: RefCell<Option<Value>>,
}

impl Mutant {
    fn fire(&self, value: Value) -> Option<Value> {
        self.inner.fragment.fired.fetch_add(1, Ordering::Relaxed);
        Some(value)
    }
}

impl Compiled for Mutant {
    fn describes(&self, program: &Program) -> bool {
        self.inner.describes(program)
    }

    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
        let fragment = self.inner.fragment;
        fragment.offered.fetch_add(1, Ordering::Relaxed);
        if self.target.as_ref().is_some_and(|t| t != name) {
            return self.inner.answer(name, args, budget);
        }
        fragment.offered_target.fetch_add(1, Ordering::Relaxed);

        // The two that answer without an honest answer to corrupt.
        match &self.mutation {
            Mutation::Answers(value) => return self.fire(Value::Int(*value)),
            Mutation::Unoffered if !fragment.holds(name) => {
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
            (Mutation::ExceedsBudget(times), None) if fragment.holds(name) => {
                let fuel = match times {
                    None => usize::MAX,
                    Some(k) => budget.saturating_mul(*k as usize),
                };
                match self.inner.run(name, args, fuel) {
                    Some(value) => self.fire(value),
                    None => None,
                }
            }
            (_, answer) => answer,
        }
    }
}

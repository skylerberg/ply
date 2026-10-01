//! The searches that discharge an obligation, and the readers of what `proof.world` decided:
//! which search each obligation goes to, and what the static prover answered, are the program's.

// `Value` shares non-`Send` payloads through `Arc` by design.
#![allow(clippy::arc_with_non_send_sync)]

pub mod concurrency;
pub mod sort;
pub mod world;

pub use sort::Sort;
pub use world::World;

use ply_eval::{DefHash, Diagnostic, Plan, Race, Seed, Span, Symbol};
use serde::Serialize;
use std::fmt;
use std::time::Duration;

/// Kept cases below which a run has concrete evidence and no coverage claim.
pub const MIN_PROPERTY_CASES: u32 = 25;

pub const DEFAULT_CASES: u32 = 200;
pub const DEFAULT_PROVE_BUDGET: u32 = 10_000;
pub const DEFAULT_SHRINK_BUDGET: u32 = 500;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    /// Concrete cases, and no coverage claim.
    Example,
    Property,
    Proved,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Example => "example",
            Tier::Property => "property",
            Tier::Proved => "proved",
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    GroundEvaluation,
    ExhaustiveEnumeration {
        domain: Symbol,
        points: u64,
    },
    /// `+`, `-`, unary `-`, multiplication by a literal, and the six comparisons.
    LinearArithmetic,
    /// `&&`, `||`, `!`, `if` at `Bool`, by case split.
    Propositional,
    CaseSplit {
        ty: Symbol,
        arms: u32,
    },
    Congruence,
    /// `C(x̄) == C(ȳ) ⟺ x̄ == ȳ`, and `C(..) != D(..)` for `C ≠ D`.
    Injectivity,
    Unfold {
        def: Symbol,
        depth: u32,
    },
    /// The claim at `binder <= 0`, then at `binder > 0` from itself at `binder - 1`, with `def`
    /// shown to terminate and unrolled.
    Induction {
        binder: Symbol,
        def: Symbol,
    },
    ExhaustiveInterleaving {
        interleavings: u32,
    },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Certificate {
    /// In application order.
    pub rules: Vec<Rule>,
    pub steps: u32,
    pub guard_satisfiable: bool,
    /// Type variables left as uninterpreted sorts, so the claim is polymorphic.
    pub sorts: Vec<Symbol>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CaseReport {
    pub generated: u32,
    pub kept: u32,
    pub rejected: u32,
    pub roots: Vec<u64>,
    pub instantiations: Vec<(Symbol, String)>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Evidence {
    Proof(Certificate),
    Cases(CaseReport),
}

impl Evidence {
    pub fn tier(&self) -> Tier {
        match self {
            Evidence::Proof(_) => Tier::Proved,
            Evidence::Cases(c) if c.kept >= MIN_PROPERTY_CASES => Tier::Property,
            Evidence::Cases(_) => Tier::Example,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Binding {
    pub name: Symbol,
    /// The binder's type as the compiler prints it.
    pub ty: String,
    pub value: ply_eval::Plain,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Counterexample {
    pub bindings: Vec<Binding>,
    pub original: Vec<Binding>,
    pub shrinks: u32,
    pub root: u64,
    pub case: u32,
    pub race: Option<Race>,
    pub sim_seed: Option<Seed>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum VacuityKind {
    ProvedUnsatisfiable,
    NoCaseKept { generated: u32 },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Vacuity {
    pub guard: Span,
    pub kind: VacuityKind,
}

#[derive(Clone, Debug)]
pub enum Gap {
    /// Checking an `ensures` calls the definition, whose row needs an unsupplied handler: the row as
    /// a report prints it, or `None` when it names nothing to hold a handler for.
    UnhandledEffect(Option<String>),
    Ungeneratable {
        param: Symbol,
        /// As the compiler prints it.
        ty: String,
    },
    /// The program's own raise; a diagnostic that is Ply's failure is a [`Discharge::Faulted`].
    Raised {
        bindings: Vec<Binding>,
        diagnostic: Box<Diagnostic>,
        /// The draw the values came from, so a program that shrinks this counterexample can
        /// regenerate them: a value of a type the program never named is not something it can hold.
        root: u64,
        case: u32,
    },
    /// The guard kept no case of a full budget, yet admits `witness`.
    GuardNotSampled {
        generated: u32,
        witness: Vec<Binding>,
    },
    /// A `law/host` under a hermetic run, with its row as a report prints it.
    ReachesHost(Option<String>),
    /// The obligation's points are not drawn one at a time, so there is no case to re-run: a
    /// concurrency law's points are interleavings that the search chooses.
    NotDrawn,
}

/// Ply's own failure while discharging a claim, as [`ply_eval::codes::is_defect`] tells it apart.
#[derive(Clone, Debug)]
pub struct Fault {
    /// The point being judged when Ply failed, or none when it failed before a point was drawn.
    pub bindings: Vec<Binding>,
    pub diagnostic: Box<Diagnostic>,
}

#[derive(Clone, Debug)]
pub enum Discharge {
    Held(Evidence),
    Refuted(Counterexample),
    Vacuous(Vacuity),
    Unattempted(Gap),
    /// Neither a verdict on the claim nor a gap in it: Ply failed rather than the program.
    Faulted(Fault),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObligationKind {
    /// Its place among the owner's `ensures` clauses.
    Ensures {
        index: usize,
    },
    Law,
}

/// One binder of a claim, numbered with the claim's other binders.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Binder {
    pub name: Symbol,
    pub sort: Sort,
    /// Its type as the compiler prints it.
    pub text: String,
}

/// Which search `proof.world` decided an obligation goes to, as far as the runtime is concerned:
/// which machine its propositions run on, and whether its points are interleavings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    /// A law over a `simulate` region: its interleavings are searched at each of its points.
    Interleave,
    /// A `law/host`, run against the host the run binds.
    Hosted,
    /// The static prover first, then the claim's points.
    Static,
}

/// A claim the program owes, as `proof.world` built it.
#[derive(Clone, Debug)]
pub struct Obligation {
    /// `spec_hash` for a clause, the law's own `DefHash` for a law.
    pub key: DefHash,
    /// `<module>.<def>` for a clause, `<module>.<label>` for a law.
    pub owner: Symbol,
    pub kind: ObligationKind,
    pub span: Span,
    /// What a point assigns: the owner's parameters for a clause, the `forall` binders for a law.
    pub binders: Vec<Binder>,
    /// A clause's `result`, which is the owner's answer and never drawn.
    pub result: Option<Binder>,
    /// The name each type variable of the binders and the result prints as, by its number.
    pub variables: Vec<Symbol>,
    /// The claim's own row as a report prints it, when it performs anything: `{sim.read}` for a
    /// concurrency law, or any row at all for a `law/host`.
    pub footprint: Option<String>,
    pub strategy: Strategy,
    /// Each guard's place: an owner's `requires` clauses, or a law's `where`.
    pub guards: Vec<Span>,
}

impl Obligation {
    /// Where a vacuity points: the first guard, or the claim.
    pub fn guard_span(&self) -> Span {
        self.guards.first().copied().unwrap_or(self.span)
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProvePlan {
    /// Per root.
    pub cases: u32,
    pub roots: Vec<u64>,
    /// Per obligation.
    pub prove_budget: u32,
    /// Candidate evaluations, not seconds, so the artifact does not vary with machine load.
    pub shrink_budget: u32,
    /// Calls per evaluation of a claim; 0 is no bound. It decides what an evaluation reports, so
    /// it keys the result: a proof means the same thing on every machine.
    pub step_budget: i64,
    pub sim: Plan,
}

impl Default for ProvePlan {
    fn default() -> ProvePlan {
        ProvePlan {
            cases: DEFAULT_CASES,
            roots: vec![0],
            prove_budget: DEFAULT_PROVE_BUDGET,
            shrink_budget: DEFAULT_SHRINK_BUDGET,
            step_budget: ply_eval::DEFAULT_STEP_BUDGET,
            sim: Plan::default(),
        }
    }
}

impl ProvePlan {
    /// So that two spellings of one plan are one cache key.
    pub fn normalized(mut self) -> ProvePlan {
        self.roots.sort_unstable();
        self.roots.dedup();
        self.sim = self.sim.normalized();
        self
    }
}

/// What a run discharged: each obligation it was asked about beside what became of it.
#[derive(Clone, Debug)]
pub struct ProveReport {
    pub obligations: Vec<(Obligation, Discharge)>,
    pub duration: Duration,
}

pub fn interleaving_proves(
    plan: &Plan,
    exploration: &ply_eval::Exploration,
    domain_enumerated: bool,
) -> bool {
    plan.mode == ply_eval::SimMode::Dpor
        && exploration.exhaustive
        && !exploration.exhausted
        && exploration.failure.is_none()
        && domain_enumerated
}

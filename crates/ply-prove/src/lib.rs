//! Obligations, and the tiers they are discharged at.

// `Value` shares non-`Send` payloads through `Arc` by design.
#![allow(clippy::arc_with_non_send_sync)]

pub mod concurrency;
pub mod domain;
pub mod property;
pub mod prove;
pub mod shrink;
pub mod sort;
pub mod world;

pub use sort::Sort;
pub use world::World;

use ply_eval::{Plan, Race, Seed};
use ply_span::{Diagnostic, Span, Symbol};
use ply_ty::DefHash;
use ply_ty::Footprint;
use serde::Serialize;
use std::fmt;
use std::time::Duration;

/// Kept cases below which a run has concrete evidence and no coverage claim.
pub const MIN_PROPERTY_CASES: u32 = 25;

pub const UNFOLD_DEPTH: u32 = 3;

pub const ENUMERATION_BOUND: u64 = 4096;

/// Past this depth only non-recursive constructors are drawn, so generation terminates.
pub const GEN_DEPTH: u32 = 4;

pub const DEFAULT_CASES: u32 = 200;
pub const DEFAULT_PROVE_BUDGET: u32 = 10_000;
pub const DEFAULT_SHRINK_BUDGET: u32 = 500;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
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

impl Rule {
    pub fn is_execution(&self) -> bool {
        matches!(
            self,
            Rule::GroundEvaluation
                | Rule::ExhaustiveEnumeration { .. }
                | Rule::ExhaustiveInterleaving { .. }
        )
    }
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Binding {
    pub name: Symbol,
    /// The binder's type as the compiler prints it.
    pub ty: String,
    pub rendered: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
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
    /// Checking an `ensures` calls the definition, whose footprint needs an unsupplied handler.
    UnhandledEffect(Footprint),
    Ungeneratable {
        param: Symbol,
        /// As the compiler prints it.
        ty: String,
    },
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
    /// A `law/host` under a hermetic run.
    ReachesHost(Footprint),
    /// The obligation's points are not drawn one at a time, so there is no case to re-run: a
    /// concurrency law's points are interleavings that the search chooses.
    NotDrawn,
}

#[derive(Clone, Debug)]
pub enum Discharge {
    Held(Evidence),
    Refuted(Counterexample),
    Vacuous(Vacuity),
    Unattempted(Gap),
}

impl Discharge {
    pub fn tier(&self) -> Option<Tier> {
        match self {
            Discharge::Held(e) => Some(e.tier()),
            _ => None,
        }
    }

    pub fn holds(&self) -> bool {
        matches!(self, Discharge::Held(_))
    }

    pub fn is_cacheable(&self) -> bool {
        self.holds()
    }

    pub fn is_plan_independent(&self) -> bool {
        self.tier() == Some(Tier::Proved)
    }
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
    /// Its type as the compiler prints it, with the letters [`Sort`]'s `Display` gives its variables.
    pub text: String,
}

impl Binder {
    pub fn new(name: &str, sort: Sort) -> Binder {
        Binder {
            name: Symbol::new(name),
            text: sort.to_string(),
            sort,
        }
    }
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
    /// The owner's parameters then `result` for a clause; the `forall` binders for a law.
    pub binders: Vec<Binder>,
    pub guarded: bool,
    /// `law/host`: the body reaches the world.
    pub host: bool,
    /// `{}`, or `{sim.read}` for a concurrency law, or any row at all for a `law/host`.
    pub footprint: Footprint,
}

impl Obligation {
    /// A law whose body reaches a `simulate` region.
    pub fn is_concurrency_law(&self) -> bool {
        matches!(self.kind, ObligationKind::Law) && !self.host && !self.footprint.is_empty()
    }

    pub fn generated(&self) -> &[Binder] {
        match self.kind {
            ObligationKind::Ensures { .. } => &self.binders[..self.binders.len().saturating_sub(1)],
            ObligationKind::Law => &self.binders,
        }
    }

    /// The return-value binder that [`Obligation::generated`] withholds.
    pub fn result_binder(&self) -> Option<&Binder> {
        match self.kind {
            ObligationKind::Ensures { .. } => self.binders.last(),
            ObligationKind::Law => None,
        }
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

#[derive(Clone, Debug)]
pub struct ProveReport {
    pub obligations: Vec<(Obligation, Discharge)>,
    pub plan: ProvePlan,
    pub duration: Duration,
}

impl ProveReport {
    pub fn count(&self, tier: Tier) -> usize {
        self.obligations
            .iter()
            .filter(|(_, d)| d.tier() == Some(tier))
            .count()
    }

    pub fn refuted(&self) -> usize {
        self.obligations
            .iter()
            .filter(|(_, d)| matches!(d, Discharge::Refuted(_)))
            .count()
    }

    pub fn vacuous(&self) -> usize {
        self.obligations
            .iter()
            .filter(|(_, d)| matches!(d, Discharge::Vacuous(_)))
            .count()
    }

    pub fn unattempted(&self) -> usize {
        self.obligations
            .iter()
            .filter(|(_, d)| matches!(d, Discharge::Unattempted(_)))
            .count()
    }

    pub fn failed(&self) -> bool {
        self.refuted() > 0 || self.vacuous() > 0
    }
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

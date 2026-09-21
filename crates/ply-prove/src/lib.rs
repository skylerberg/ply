//! Obligations, and the tiers they are discharged at.

// `Value` shares non-`Send` payloads through `Arc` by design.
#![allow(clippy::arc_with_non_send_sync)]

pub mod concurrency;
pub mod domain;
pub mod key;
pub mod property;
pub mod prove;
pub mod shrink;

use ply_eval::{Plan, Race, Seed};
use ply_span::{Diagnostic, Span, Symbol};
use ply_ty::DefHash;
use ply_ty::{Footprint, LawBinder, Resource, Type};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Certificate {
    /// In application order.
    pub rules: Vec<Rule>,
    pub steps: u32,
    pub guard_satisfiable: bool,
    /// Type variables left as uninterpreted sorts, so the claim is polymorphic.
    pub sorts: Vec<Symbol>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct CaseReport {
    pub generated: u32,
    pub kept: u32,
    pub rejected: u32,
    pub roots: Vec<u64>,
    pub instantiations: Vec<(Symbol, Type)>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "evidence")]
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

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Binding {
    pub name: Symbol,
    pub ty: Type,
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

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum VacuityKind {
    ProvedUnsatisfiable,
    NoCaseKept { generated: u32 },
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
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
        ty: Type,
    },
    Raised {
        bindings: Vec<Binding>,
        diagnostic: Box<Diagnostic>,
    },
    /// The guard kept no case of a full budget, yet admits `witness`.
    GuardNotSampled {
        generated: u32,
        witness: Vec<Binding>,
    },
    /// A `law/host` under a hermetic run.
    ReachesHost(Footprint),
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

/// What a definition leaves alone, from its checked footprint.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "frame")]
pub enum Frame {
    /// The `ensures` is a total specification: the result depends only on the arguments.
    Pure,
    Writes(BTreeSet<(Symbol, Resource)>),
}

/// A read changes nothing, so it does not narrow a frame.
pub fn frame_of(footprint: &Footprint) -> Frame {
    let writes: BTreeSet<(Symbol, Resource)> = footprint
        .atoms()
        .filter(|a| a.mode == ply_ty::Mode::Write)
        .map(|a| (a.effect.clone(), a.resource.clone()))
        .collect();
    if writes.is_empty() {
        Frame::Pure
    } else {
        Frame::Writes(writes)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ObligationKind {
    Ensures { index: usize },
    Law,
}

#[derive(Clone, Debug)]
pub struct Obligation {
    /// `spec_hash` for a clause, the law's own `DefHash` for a law.
    pub key: DefHash,
    /// `<module>.<def>` for a clause, `<module>.<label>` for a law.
    pub owner: Symbol,
    pub kind: ObligationKind,
    pub span: Span,
    pub frame: Frame,
    /// The owner's parameters then `result` for a clause; the `forall` binders for a law.
    pub binders: Vec<LawBinder>,
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

    pub fn generated(&self) -> &[LawBinder] {
        match self.kind {
            ObligationKind::Ensures { .. } => &self.binders[..self.binders.len().saturating_sub(1)],
            ObligationKind::Law => &self.binders,
        }
    }

    /// The return-value binder that [`Obligation::generated`] withholds.
    pub fn result_binder(&self) -> Option<&LawBinder> {
        match self.kind {
            ObligationKind::Ensures { .. } => self.binders.last(),
            ObligationKind::Law => None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Coverage {
    pub definitions: usize,
    /// Carries an `ensures` that holds, or is named directly by a law that holds.
    pub covered: usize,
    /// Sorted, so two runs produce one artifact.
    pub uncovered: Vec<Symbol>,
    pub by_tier: BTreeMap<Tier, usize>,
}

impl Coverage {
    pub fn uncovered_count(&self) -> usize {
        self.definitions.saturating_sub(self.covered)
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

    pub fn digest(&self) -> [u8; 32] {
        let plan = self.clone().normalized();
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"ply.prove.plan.1");
        hasher.update(&plan.cases.to_le_bytes());
        hasher.update(&plan.prove_budget.to_le_bytes());
        hasher.update(&plan.step_budget.to_le_bytes());
        hasher.update(&(plan.roots.len() as u32).to_le_bytes());
        for root in &plan.roots {
            hasher.update(&root.to_le_bytes());
        }
        hasher.update(&plan.sim.digest());
        *hasher.finalize().as_bytes()
    }
}

#[derive(Clone, Debug)]
pub struct ProveReport {
    pub obligations: Vec<(Obligation, Discharge)>,
    pub coverage: Coverage,
    pub plan: ProvePlan,
    pub cached: usize,
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
